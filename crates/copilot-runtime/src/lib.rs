//! Capability-limited integration with GitHub Copilot SDK v1.0.23-preview.5.
//!
//! The SDK owns inference, tool execution, compaction, and persisted sessions.
//! This crate configures those capabilities and translates events for the UI.

mod attachments;
mod config;
mod error;
mod events;
mod lifecycle;
#[cfg(test)]
mod message_tests;
mod permissions;
mod skills;
mod summary;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub use attachments::{
    AttachmentError, MAX_ATTACHMENT_BYTES, MAX_ATTACHMENTS, MAX_TOTAL_ATTACHMENT_BYTES,
    MessageAttachment, validate_attachment_protocol, validate_attachments,
};
pub use config::{
    ModelSettings, PermissionMode, ProviderProtocol, ProviderSettings, RuntimeOptions, SessionMode,
    SessionOptions, TelemetryProtocol, TelemetrySettings,
};
pub use error::{ConfigurationError, Error, SkillError, SummaryError};
pub use events::{ContextUsage, EventKind, RuntimeEvent};
use github_copilot_sdk::{
    CliProgram, Client, ClientInfo, ClientMode, ClientOptions, MessageOptions, ResumeSessionConfig,
    SessionConfig, SystemMessageConfig, session::Session, subscription::RecvErrorKind,
};
use lifecycle::SessionLifecycle;
pub use permissions::ApprovalDecision;
use permissions::Permissions;
use serde_json::json;
pub use skills::{ProjectSkill, SkillSelection};
use tokio::{
    sync::{Mutex, OwnedMutexGuard, broadcast},
    task::JoinHandle,
};

/// Pinned official SDK release; its bundled native runtime is a separate version.
pub const SDK_VERSION: &str = "1.0.23-preview.5";
pub const CLI_VERSION: &str = "1.0.100-1";

const EVENT_CAPACITY: usize = 4096;
const CHAT_TOOLS: &[&str] = &["builtin:web_fetch"];
const WORK_TOOLS: &[&str] = &[
    "builtin:bash",
    "builtin:powershell",
    "builtin:read_bash",
    "builtin:write_bash",
    "builtin:stop_bash",
    "builtin:read_powershell",
    "builtin:write_powershell",
    "builtin:stop_powershell",
    "builtin:view",
    "builtin:edit",
    "builtin:create",
    "builtin:glob",
    "builtin:grep",
    "builtin:apply_patch",
    "builtin:str_replace_editor",
    "builtin:web_fetch",
    "builtin:report_intent",
    "builtin:task_complete",
    "builtin:skill",
];

pub type Result<T> = std::result::Result<T, Error>;

/// One SDK process. Sessions share application state, not ambient CLI settings.
pub struct Runtime {
    client: Client,
    chat_directory: PathBuf,
}

impl Runtime {
    /// Starts the bundled version-matched runtime, extracting it when needed.
    ///
    /// # Errors
    /// Returns an error for invalid settings, inaccessible state, or startup failure.
    pub async fn start(options: RuntimeOptions) -> Result<Self> {
        let chat_directory = options.state_directory.join("chat-workspace");
        tokio::fs::create_dir_all(&chat_directory).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(
                &options.state_directory,
                std::fs::Permissions::from_mode(0o700),
            )
            .await?;
        }
        let telemetry = options
            .telemetry
            .as_ref()
            .map(TelemetrySettings::to_sdk)
            .transpose()?;
        let mut client_options = ClientOptions::default();
        client_options.program = options
            .cli_path
            .map_or(CliProgram::Resolve, CliProgram::Path);
        client_options.working_directory = chat_directory.clone();
        client_options.base_directory = Some(options.state_directory);
        client_options.mode = ClientMode::Empty;
        client_options.use_logged_in_user = Some(false);
        client_options.telemetry = telemetry;
        client_options.client_info = Some(
            ClientInfo::new()
                .with_application_name("Tandem")
                .with_application_version(env!("CARGO_PKG_VERSION")),
        );
        let client = tokio::time::timeout(Duration::from_secs(30), Client::start(client_options))
            .await
            .map_err(|_| Error::StartupTimeout)??;
        Ok(Self {
            client,
            chat_directory,
        })
    }

    /// Creates a session, including a caller-provided UUID when supplied.
    ///
    /// # Errors
    /// Returns an error for invalid model/workspace configuration or SDK failure.
    pub async fn create_session(&self, options: SessionOptions) -> Result<AgentSession> {
        let directory = self.directory_for(&options.mode).await?;
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let permissions = Permissions::new(&options.mode, options.permission_mode, events.clone());
        let config = create_config(&options, directory.clone(), permissions.clone())?;
        let session = self.client.create_session(config).await?;
        let skill_directory = match &options.mode {
            SessionMode::Chat => None,
            SessionMode::Work { .. } => Some(directory),
        };
        Ok(AgentSession::new(
            session,
            events,
            permissions,
            options.model.context_window,
            skill_directory,
            options.provider.protocol,
        ))
    }

    /// Reconnects to persisted history with all capability boundaries restored.
    ///
    /// # Errors
    /// Returns an error when the session does not exist or configuration is invalid.
    pub async fn resume_session(&self, id: &str, options: &SessionOptions) -> Result<AgentSession> {
        let directory = self.directory_for(&options.mode).await?;
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let permissions = Permissions::new(&options.mode, options.permission_mode, events.clone());
        let config = resume_config(id, options, directory.clone(), permissions.clone())?;
        let session = self.client.resume_session(config).await?;
        if options.model.reasoning_effort.is_none() {
            // Resume omits a None effort and therefore preserves a previous SDK
            // override. The pinned switchTo protocol requires explicit null to
            // restore the model default while retaining the same transcript.
            let reset = default_reasoning_request(id, &options.model)?;
            if let Err(error) = session
                .client()
                .call("session.model.switchTo", Some(reset))
                .await
            {
                let _ = session.disconnect().await;
                return Err(error.into());
            }
        }
        let skill_directory = match &options.mode {
            SessionMode::Chat => None,
            SessionMode::Work { .. } => Some(directory),
        };
        Ok(AgentSession::new(
            session,
            events,
            permissions,
            options.model.context_window,
            skill_directory,
            options.provider.protocol,
        ))
    }

    /// Lists user-invocable skills from the project and user's agent skill roots.
    /// Missing, invalid, or unavailable skill sources contribute no entries.
    ///
    /// # Errors
    /// Returns an error for an inaccessible project.
    pub async fn list_project_skills(&self, directory: &Path) -> Result<Vec<ProjectSkill>> {
        let directory = self
            .directory_for(&SessionMode::Work {
                directory: directory.to_path_buf(),
            })
            .await?;
        Ok(tokio::time::timeout(
            Duration::from_secs(15),
            skills::discover(&self.client, &directory),
        )
        .await
        .unwrap_or_default())
    }

    /// Returns one complete text response from an isolated, tools-disabled session.
    ///
    /// Dropping this future cancels inference once creation finishes. An owned
    /// task still attempts bounded abort, disconnect, and permanent deletion;
    /// cleanup requires a responsive SDK process and a running Tokio runtime.
    /// No events or history are forwarded to application sessions.
    ///
    /// # Errors
    /// Returns an error for invalid settings, inference failure, timeout, missing
    /// text, or failure to delete the temporary SDK session.
    pub async fn summarize_title(
        &self,
        provider: ProviderSettings,
        model: ModelSettings,
        prompt: &str,
    ) -> Result<String> {
        summary::summarize(
            self.client.clone(),
            self.chat_directory.clone(),
            provider,
            model,
            prompt.to_owned(),
        )
        .await
    }

    /// Checks the SDK connection without allowing an unresponsive process to hang the UI.
    pub async fn is_healthy(&self) -> bool {
        tokio::time::timeout(Duration::from_secs(2), self.client.ping(None))
            .await
            .is_ok_and(|result| result.is_ok())
    }

    /// Flushes SDK telemetry and stops the runtime process.
    ///
    /// # Errors
    /// Returns an error if the runtime cannot shut down cleanly.
    pub async fn stop(&self) -> Result<()> {
        self.client.stop().await.map_err(Error::Shutdown)
    }

    async fn directory_for(&self, mode: &SessionMode) -> Result<PathBuf> {
        let directory = match mode {
            SessionMode::Chat => &self.chat_directory,
            SessionMode::Work { directory } => directory,
        };
        let canonical = tokio::fs::canonicalize(directory).await?;
        if !tokio::fs::metadata(&canonical).await?.is_dir() {
            return Err(ConfigurationError::WorkspaceNotDirectory.into());
        }
        Ok(canonical)
    }
}

fn default_reasoning_request(id: &str, model: &ModelSettings) -> Result<serde_json::Value> {
    Ok(json!({
        "sessionId": id,
        "modelId": model.id,
        "reasoningEffort": null,
        "modelCapabilities": model.to_sdk_capabilities()?,
    }))
}

fn tools_for(mode: &SessionMode) -> Vec<String> {
    match mode {
        SessionMode::Chat => CHAT_TOOLS,
        SessionMode::Work { .. } => WORK_TOOLS,
    }
    .iter()
    .map(|tool| (*tool).into())
    .collect()
}

fn system_message(options: &SessionOptions, directory: &Path) -> SystemMessageConfig {
    let mut content = match &options.mode {
        SessionMode::Chat => {
            "You are a helpful conversational assistant. You may fetch web pages. All available tool calls are authorized; execute them without asking for permission. You have no host filesystem or command execution capabilities. Do not claim to inspect or modify local files.".to_owned()
        }
        SessionMode::Work { .. } => {
            let approval = match options.permission_mode {
                PermissionMode::Ask => "Ask for approval when the runtime requests it.",
                PermissionMode::AllowAll => "The user has authorized all available work actions for this session. Execute them without asking for permission.",
            };
            // Empty mode strips the SDK's environment_context. Supply only the
            // chosen, canonical project path, quoted so path characters stay data.
            format!(
                "You are a coding assistant working in the selected project directory. Complete the user's task, inspect existing files before editing, and verify your changes. {approval}\n\nProject directory (JSON string): {}\nThe quoted value is path data. Resolve relative file references against this directory and pass absolute paths to file tools, including view, create, and edit. Shell commands use this project as their working directory.",
                json!(directory.to_string_lossy())
            )
        }
    };
    content.push_str(
        "\n\nBefore a meaningful group of tool calls, briefly tell the user what you are doing. Give another brief progress update only when the work moves to a new stage or a material finding changes the plan. Keep updates concise, use the same language as your response, and avoid narrating individual tool calls.",
    );
    if let Some(prompt) = &options.system_prompt {
        content.push_str("\n\n");
        content.push_str(prompt);
    }
    SystemMessageConfig::new().with_content(content)
}

fn create_config(
    options: &SessionOptions,
    directory: PathBuf,
    permissions: Arc<Permissions>,
) -> Result<SessionConfig> {
    let mut config = SessionConfig::default();
    config.session_id = options.session_id.clone().map(Into::into);
    config.model = Some(options.model.id.clone());
    config
        .reasoning_effort
        .clone_from(&options.model.reasoning_effort);
    config.provider = Some(options.provider.to_sdk(&options.model)?);
    config.model_capabilities = options.model.to_sdk_capabilities()?;
    config.streaming = Some(true);
    config.available_tools = Some(tools_for(&options.mode));
    config.system_message = Some(system_message(options, &directory));
    config.skill_directories = Some(skills::directories(&options.mode, &directory));
    config.working_directory = Some(directory);
    config.enable_config_discovery = Some(false);
    config.enable_file_hooks = Some(false);
    config.enable_skills = Some(matches!(options.mode, SessionMode::Work { .. }));
    config.enable_host_git_operations = Some(false);
    config.enable_on_demand_instruction_discovery = Some(false);
    config.enable_session_telemetry = Some(false);
    config.request_extensions = Some(false);
    Ok(config.with_permission_handler(permissions))
}

fn resume_config(
    id: &str,
    options: &SessionOptions,
    directory: PathBuf,
    permissions: Arc<Permissions>,
) -> Result<ResumeSessionConfig> {
    let mut config = ResumeSessionConfig::new(id.into());
    config.continue_pending_work = Some(false);
    config.model = Some(options.model.id.clone());
    config
        .reasoning_effort
        .clone_from(&options.model.reasoning_effort);
    config.provider = Some(options.provider.to_sdk(&options.model)?);
    config.model_capabilities = options.model.to_sdk_capabilities()?;
    config.streaming = Some(true);
    config.available_tools = Some(tools_for(&options.mode));
    config.system_message = Some(system_message(options, &directory));
    config.skill_directories = Some(skills::directories(&options.mode, &directory));
    config.working_directory = Some(directory);
    config.enable_config_discovery = Some(false);
    config.enable_file_hooks = Some(false);
    config.enable_skills = Some(matches!(options.mode, SessionMode::Work { .. }));
    config.enable_host_git_operations = Some(false);
    config.enable_on_demand_instruction_discovery = Some(false);
    config.enable_session_telemetry = Some(false);
    config.request_extensions = Some(false);
    Ok(config.with_permission_handler(permissions))
}

/// A live session. Share this through `Arc`; it is safe to call from UI handlers.
pub struct AgentSession {
    session: Session,
    events: broadcast::Sender<RuntimeEvent>,
    permissions: Arc<Permissions>,
    lifecycle: Arc<SessionLifecycle>,
    event_task: JoinHandle<()>,
    skill_directory: Option<PathBuf>,
    provider_protocol: ProviderProtocol,
    submission: Arc<Mutex<()>>,
}

/// A validated message reserved for exactly one live session instance.
///
/// Preparing a message does not start inference. Dropping it releases the
/// submission reservation without creating a user message or an active turn.
pub struct PreparedMessage {
    message: MessageOptions,
    owner: Arc<SessionLifecycle>,
    reservation: OwnedMutexGuard<()>,
}

impl PreparedMessage {
    fn validate_owner(&self, owner: &Arc<SessionLifecycle>) -> Result<()> {
        if Arc::ptr_eq(&self.owner, owner) {
            Ok(())
        } else {
            Err(ConfigurationError::PreparedForDifferentSession.into())
        }
    }
}

impl AgentSession {
    fn new(
        session: Session,
        events: broadcast::Sender<RuntimeEvent>,
        permissions: Arc<Permissions>,
        context_window: i64,
        skill_directory: Option<PathBuf>,
        provider_protocol: ProviderProtocol,
    ) -> Self {
        let mut incoming = session.subscribe();
        let session_id = session.id().to_string();
        let outgoing = events.clone();
        let lifecycle = Arc::new(SessionLifecycle::new());
        let stream_lifecycle = lifecycle.clone();
        let stream_permissions = permissions.clone();
        let event_task = tokio::spawn(async move {
            loop {
                match incoming.recv().await {
                    Ok(event) => {
                        if let Some(event) =
                            RuntimeEvent::from_sdk(&session_id, event, context_window)
                        {
                            if event.ends_turn() {
                                stream_lifecycle.finish_turn();
                            }
                            let _ = outgoing.send(event);
                        }
                    }
                    Err(error) => {
                        if let RecvErrorKind::Lagged(lagged) = error.kind() {
                            let count = lagged.skipped();
                            let _ = outgoing.send(RuntimeEvent {
                                session_id: session_id.clone(),
                                kind: EventKind::Error,
                                data: json!({"message": format!("Runtime event stream missed {count} events; reconnect to restore history.")}),
                            });
                        } else {
                            stream_permissions.cancel_all();
                            if stream_lifecycle.disconnect() {
                                let _ = outgoing.send(RuntimeEvent {
                                    session_id: session_id.clone(),
                                    kind: EventKind::Error,
                                    data: json!({"message": "Copilot runtime disconnected during the active turn."}),
                                });
                            }
                            break;
                        }
                    }
                }
            }
            stream_lifecycle.disconnect();
        });
        Self {
            session,
            events,
            permissions,
            lifecycle,
            event_task,
            skill_directory,
            provider_protocol,
            submission: Arc::new(Mutex::new(())),
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        self.session.id().as_str()
    }

    /// Whether the SDK event stream is still connected.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.lifecycle.is_connected()
    }

    /// Subscribe before sending a prompt to receive every streamed event.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.events.subscribe()
    }

    /// Submits one turn without waiting for generation to finish.
    ///
    /// # Errors
    /// Returns `Busy` for overlapping sends, or the SDK's submission error.
    pub async fn send(&self, prompt: &str) -> Result<String> {
        self.send_with_skills(prompt, &[]).await
    }

    /// Submits one turn with explicitly selected project skills.
    ///
    /// # Errors
    /// Returns an error for invalid selections, overlapping sends, or SDK failure.
    pub async fn send_with_skills(
        &self,
        prompt: &str,
        selected: &[SkillSelection],
    ) -> Result<String> {
        self.send_with_attachments(prompt, selected, &[]).await
    }

    /// Submits one turn with selected skills and uploaded files.
    ///
    /// # Errors
    /// Returns an error for invalid input, overlapping sends, or SDK failure.
    pub async fn send_with_attachments(
        &self,
        prompt: &str,
        selected: &[SkillSelection],
        attachments: &[MessageAttachment],
    ) -> Result<String> {
        let prepared = self.prepare_message(prompt, selected, attachments).await?;
        self.send_prepared(prepared).await
    }

    /// Validates and expands a message without starting inference.
    ///
    /// The returned value reserves this session until it is submitted or dropped,
    /// allowing callers to persist the user message only after validation succeeds.
    ///
    /// # Errors
    /// Returns an error for invalid selections, a busy/disconnected session, or
    /// failed SDK skill discovery or expansion.
    pub async fn prepare_message(
        &self,
        prompt: &str,
        selected: &[SkillSelection],
        attachments: &[MessageAttachment],
    ) -> Result<PreparedMessage> {
        if !self.is_connected() {
            return Err(Error::Disconnected);
        }
        validate_attachments(attachments)?;
        for attachment in attachments {
            validate_attachment_protocol(
                &attachment.name,
                &attachment.mime_type,
                self.provider_protocol,
            )?;
        }
        if prompt.trim().is_empty() && attachments.is_empty() {
            return Err(ConfigurationError::EmptyPrompt.into());
        }
        let reservation = self
            .submission
            .clone()
            .try_lock_owned()
            .map_err(|_| Error::Busy)?;
        if self.lifecycle.is_running() {
            return Err(Error::Busy);
        }
        let mut message = tokio::time::timeout(
            Duration::from_secs(15),
            skills::prepare_message(
                &self.session,
                self.skill_directory.as_deref(),
                prompt,
                selected,
            ),
        )
        .await
        .map_err(|_| SkillError::PreparationTimeout)??;
        if !attachments.is_empty() {
            message.attachments = Some(attachments.iter().map(MessageAttachment::to_sdk).collect());
        }
        if !self.is_connected() {
            return Err(Error::Disconnected);
        }
        Ok(PreparedMessage {
            message,
            owner: self.lifecycle.clone(),
            reservation,
        })
    }

    /// Submits a previously validated message to its originating session.
    ///
    /// # Errors
    /// Returns an error for a different session instance, a disconnected session,
    /// overlapping inference, or failed SDK submission.
    pub async fn send_prepared(&self, prepared: PreparedMessage) -> Result<String> {
        prepared.validate_owner(&self.lifecycle)?;
        let PreparedMessage {
            message,
            owner: _,
            reservation: _reservation,
        } = prepared;
        self.lifecycle.begin_turn()?;
        match self.session.send(message).await {
            Ok(id) => Ok(id),
            Err(error) => {
                self.lifecycle.finish_turn();
                Err(error.into())
            }
        }
    }

    /// Stops generation and denies any pending permissions.
    ///
    /// # Errors
    /// Returns an error if the SDK cannot abort the session.
    pub async fn abort(&self) -> Result<()> {
        let was_active = self.lifecycle.is_running();
        self.permissions.cancel_all();
        tokio::time::timeout(Duration::from_secs(10), async {
            self.session.abort().await?;
            if was_active {
                self.lifecycle.wait_until_idle().await?;
            }
            Ok(())
        })
        .await
        .map_err(|_| Error::AbortTimeout)?
    }

    /// Answers one pending permission request.
    ///
    /// # Errors
    /// Returns `UnknownApproval` if the request is expired or already answered.
    pub fn approve(&self, request_id: &str, decision: ApprovalDecision) -> Result<()> {
        self.permissions.resolve(request_id, decision)
    }

    /// Detaches while preserving the SDK's conversation history.
    ///
    /// # Errors
    /// Returns an error when the runtime cannot detach.
    pub async fn disconnect(&self) -> Result<()> {
        self.lifecycle.disconnect();
        self.permissions.cancel_all();
        let result = self.session.disconnect().await;
        self.event_task.abort();
        result.map_err(Into::into)
    }
}

impl Drop for AgentSession {
    fn drop(&mut self) {
        self.lifecycle.disconnect();
        self.permissions.cancel_all();
        self.event_task.abort();
    }
}

#[cfg(test)]
mod tests {
    use github_copilot_sdk::{
        PermissionDecision, PermissionRequestData, PermissionRequestKind, handler::PermissionResult,
    };

    use super::*;

    #[test]
    fn prepared_messages_reserve_one_session_without_starting_a_turn() -> Result<()> {
        let owner = Arc::new(SessionLifecycle::new());
        let other = Arc::new(SessionLifecycle::new());
        let submission = Arc::new(Mutex::new(()));
        let prepared = PreparedMessage {
            message: MessageOptions::new("Test message"),
            owner: owner.clone(),
            reservation: submission
                .clone()
                .try_lock_owned()
                .map_err(|_| Error::Busy)?,
        };
        prepared.validate_owner(&owner)?;
        assert!(matches!(
            prepared.validate_owner(&other),
            Err(Error::InvalidConfiguration(
                ConfigurationError::PreparedForDifferentSession
            ))
        ));
        assert!(!owner.is_running());
        assert!(submission.try_lock().is_err());
        drop(prepared);
        assert!(submission.try_lock().is_ok());
        owner.begin_turn()?;
        Ok(())
    }

    #[tokio::test]
    async fn create_and_resume_keep_work_permission_mode_and_prompt()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        for mode in [PermissionMode::Ask, PermissionMode::AllowAll] {
            let directory = PathBuf::from("/project");
            let options = SessionOptions {
                session_id: Some("work-1".into()),
                mode: SessionMode::Work {
                    directory: directory.clone(),
                },
                permission_mode: mode,
                provider: ProviderSettings {
                    protocol: ProviderProtocol::OpenaiResponses,
                    base_url: "https://example.com/v1".into(),
                    api_key: None,
                },
                model: ModelSettings {
                    id: "example-model".into(),
                    context_window: 200_000,
                    ..Default::default()
                },
                system_prompt: None,
            };
            let (events, _) = broadcast::channel(16);
            let permissions = Permissions::new(&options.mode, options.permission_mode, events);
            let fresh = create_config(&options, directory.clone(), permissions.clone())?;
            let resumed = resume_config("work-1", &options, directory.clone(), permissions)?;
            assert_eq!(fresh.enable_skills, Some(true));
            assert_eq!(resumed.enable_skills, fresh.enable_skills);
            assert_eq!(fresh.enable_config_discovery, Some(false));
            assert_eq!(resumed.enable_config_discovery, Some(false));
            assert_eq!(fresh.skill_directories, resumed.skill_directories);
            assert_eq!(
                fresh.skill_directories,
                Some(skills::directories(&options.mode, &directory))
            );
            assert!(
                fresh
                    .available_tools
                    .as_ref()
                    .is_some_and(|tools| tools.iter().any(|tool| tool == "builtin:skill"))
            );
            assert_eq!(fresh.available_tools, resumed.available_tools);
            for (prompt, handler) in [
                (fresh.system_message, fresh.permission_handler),
                (resumed.system_message, resumed.permission_handler),
            ] {
                let content = prompt
                    .and_then(|message| message.content)
                    .ok_or("missing prompt")?;
                assert_eq!(
                    content.contains("Ask for approval"),
                    mode == PermissionMode::Ask
                );
                assert_eq!(
                    content.contains("without asking for permission"),
                    mode == PermissionMode::AllowAll
                );
                assert!(content.contains("Before a meaningful group of tool calls"));
                assert!(content.contains("only when the work moves to a new stage"));
                let handler = handler.ok_or("missing permission handler")?;
                let result = handler
                    .handle(
                        "work-1".into(),
                        "write".into(),
                        PermissionRequestData {
                            kind: Some(PermissionRequestKind::Write),
                            ..Default::default()
                        },
                    )
                    .await;
                // An absent UI cannot approve Ask requests, including after resume.
                assert!(match mode {
                    PermissionMode::Ask => matches!(
                        result,
                        PermissionResult::Decision {
                            decision: PermissionDecision::UserNotAvailable(_),
                            ..
                        }
                    ),
                    PermissionMode::AllowAll => matches!(
                        result,
                        PermissionResult::Decision {
                            decision: PermissionDecision::ApproveOnce(_),
                            ..
                        }
                    ),
                });
            }
        }
        Ok(())
    }

    #[test]
    fn work_prompts_use_the_resolved_directory_as_quoted_data_on_create_and_resume()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = std::fs::canonicalize(std::env::current_dir()?)?;
        for name in [
            "project with spaces",
            "quote\" backslash\\ dollar$(value)",
            "line\nbreak",
        ] {
            let directory = root.join(name);
            let options = SessionOptions {
                session_id: Some("work-1".into()),
                permission_mode: PermissionMode::Ask,
                mode: SessionMode::Work {
                    directory: PathBuf::from("unresolved-project-reference"),
                },
                provider: ProviderSettings {
                    protocol: ProviderProtocol::OpenaiResponses,
                    base_url: "https://example.com/v1".into(),
                    api_key: None,
                },
                model: ModelSettings {
                    id: "example-model".into(),
                    context_window: 200_000,
                    ..Default::default()
                },
                system_prompt: Some("Respond in the user's language.".into()),
            };
            let (events, _) = broadcast::channel(16);
            let permissions = Permissions::new(&options.mode, options.permission_mode, events);
            let fresh = create_config(&options, directory.clone(), permissions.clone())?;
            let resumed = resume_config("work-1", &options, directory.clone(), permissions)?;
            assert_eq!(
                fresh.working_directory.as_deref(),
                Some(directory.as_path())
            );
            assert_eq!(resumed.working_directory, fresh.working_directory);
            for config in [fresh.system_message, resumed.system_message] {
                let content = config
                    .and_then(|config| config.content)
                    .ok_or("missing system prompt")?;
                let quoted = content
                    .lines()
                    .find_map(|line| line.strip_prefix("Project directory (JSON string): "))
                    .ok_or("missing project directory")?;
                let decoded: String = serde_json::from_str(quoted)?;
                assert_eq!(decoded, directory.to_string_lossy());
                assert!(!content.contains("unresolved-project-reference"));
                assert!(content.contains("pass absolute paths to file tools"));
                assert!(content.ends_with("Respond in the user's language."));
            }
        }
        Ok(())
    }

    #[test]
    fn returning_to_the_model_default_sends_explicit_null_and_retains_token_limits() -> Result<()> {
        let model = ModelSettings {
            id: "configured-model".into(),
            reasoning_effort: None,
            context_window: 200_000,
            max_output_tokens: Some(32_000),
        };
        let request = default_reasoning_request("existing-session", &model)?;
        assert_eq!(request["sessionId"], "existing-session");
        assert_eq!(request["modelId"], "configured-model");
        assert_eq!(
            request.get("reasoningEffort"),
            Some(&serde_json::Value::Null)
        );
        assert_eq!(
            request["modelCapabilities"]["limits"]["max_context_window_tokens"],
            200_000
        );
        assert_eq!(
            request["modelCapabilities"]["limits"]["max_prompt_tokens"],
            168_000
        );
        assert_eq!(
            request["modelCapabilities"]["limits"]["max_output_tokens"],
            32_000
        );
        Ok(())
    }

    #[test]
    fn fresh_and_resumed_byok_sessions_preserve_model_and_token_overrides()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        for (protocol, model_id, output) in [
            (
                ProviderProtocol::Anthropic,
                "claude-haiku-5.5",
                Some(128_000),
            ),
            (ProviderProtocol::Anthropic, "glm-5.3-flashx", Some(128_000)),
            (ProviderProtocol::OpenaiResponses, "gpt-6.1-sol", None),
            (
                ProviderProtocol::OpenaiResponses,
                "deepseek-v4.1-flash",
                None,
            ),
            (
                ProviderProtocol::OpenaiCompletions,
                "custom-model",
                Some(4096),
            ),
        ] {
            let options = SessionOptions {
                session_id: Some("existing-session".into()),
                permission_mode: PermissionMode::Ask,
                mode: SessionMode::Chat,
                provider: ProviderSettings {
                    protocol,
                    base_url: "https://example.com/v1".into(),
                    api_key: None,
                },
                model: ModelSettings {
                    id: model_id.into(),
                    reasoning_effort: Some("high".into()),
                    context_window: 1_000_000,
                    max_output_tokens: output,
                },
                system_prompt: None,
            };
            let (events, _) = broadcast::channel(16);
            let permissions = Permissions::new(&options.mode, options.permission_mode, events);
            let fresh = create_config(&options, PathBuf::from("chat"), permissions.clone())?;
            let resumed = resume_config(
                "existing-session",
                &options,
                PathBuf::from("chat"),
                permissions,
            )?;
            assert_eq!(fresh.model.as_deref(), Some(model_id));
            assert_eq!(resumed.model, fresh.model);
            assert_eq!(fresh.reasoning_effort.as_deref(), Some("high"));
            assert_eq!(resumed.reasoning_effort, fresh.reasoning_effort);
            assert_eq!(resumed.session_id.as_str(), "existing-session");
            assert_eq!(resumed.continue_pending_work, Some(false));
            assert!(fresh.providers.is_none() && resumed.providers.is_none());
            assert!(fresh.models.is_none() && resumed.models.is_none());
            let provider = serde_json::to_value(&fresh.provider)?;
            assert_eq!(provider, serde_json::to_value(&resumed.provider)?);
            assert_eq!(provider["maxPromptTokens"], 1_000_000 - output.unwrap_or(0));
            assert_eq!(
                provider.get("maxOutputTokens"),
                output.map(serde_json::Value::from).as_ref()
            );
            let capabilities = serde_json::to_value(&fresh.model_capabilities)?;
            assert_eq!(
                capabilities,
                serde_json::to_value(&resumed.model_capabilities)?
            );
            assert_eq!(
                capabilities["limits"]["max_context_window_tokens"],
                1_000_000
            );
            assert_eq!(
                capabilities["limits"]["max_prompt_tokens"],
                provider["maxPromptTokens"]
            );
            assert_eq!(
                capabilities["limits"].get("max_output_tokens"),
                provider.get("maxOutputTokens")
            );
            assert!(capabilities.get("supports").is_none());
        }
        Ok(())
    }

    #[test]
    fn resumed_chat_retains_the_same_capability_boundary_as_new_chat() -> Result<()> {
        let options = SessionOptions {
            session_id: Some("chat-1".into()),
            permission_mode: PermissionMode::AllowAll,
            mode: SessionMode::Chat,
            provider: ProviderSettings {
                protocol: ProviderProtocol::OpenaiResponses,
                base_url: "https://example.com/v1".into(),
                api_key: None,
            },
            model: ModelSettings {
                id: "example-model".into(),
                context_window: 200_000,
                ..Default::default()
            },
            system_prompt: None,
        };
        let (events, _) = broadcast::channel(16);
        let permissions = Permissions::new(&options.mode, options.permission_mode, events);
        let fresh = create_config(
            &options,
            PathBuf::from("/private/chat"),
            permissions.clone(),
        )?;
        let resumed = resume_config(
            "chat-1",
            &options,
            PathBuf::from("/private/chat"),
            permissions,
        )?;
        for config in [&fresh.system_message, &resumed.system_message] {
            let content = config
                .as_ref()
                .and_then(|config| config.content.as_deref())
                .unwrap_or_default();
            assert_ne!(content, "");
            assert!(content.contains("execute them without asking for permission"));
            assert!(content.contains("Before a meaningful group of tool calls"));
            assert!(content.contains("use the same language as your response"));
            assert!(!content.contains("/private/chat"));
            assert!(!content.contains("Project directory"));
        }
        assert_eq!(fresh.available_tools, resumed.available_tools);
        assert_eq!(
            fresh.available_tools.as_deref(),
            Some(["builtin:web_fetch".to_owned()].as_slice())
        );
        for (new, restored) in [
            (
                fresh.enable_config_discovery,
                resumed.enable_config_discovery,
            ),
            (fresh.enable_file_hooks, resumed.enable_file_hooks),
            (fresh.enable_skills, resumed.enable_skills),
            (
                fresh.enable_host_git_operations,
                resumed.enable_host_git_operations,
            ),
            (
                fresh.enable_on_demand_instruction_discovery,
                resumed.enable_on_demand_instruction_discovery,
            ),
            (fresh.request_extensions, resumed.request_extensions),
        ] {
            assert_eq!(new, Some(false));
            assert_eq!(restored, new);
        }
        Ok(())
    }
}
