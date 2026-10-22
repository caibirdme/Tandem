use std::sync::{Arc, Weak};

use copilot_runtime::{
    MessageAttachment, SessionMode, SessionOptions, SkillSelection, validate_attachment_protocol,
    validate_attachments,
};
use serde_json::json;
use tandem_remote::RemoteTransport;
use tokio::{
    sync::{Mutex, watch},
    task::JoinHandle,
};
use uuid::Uuid;

use crate::{
    App, Error, Result,
    agent::{AgentEvents, AgentHandle, LiveSession, TurnCompletion},
    app::now,
    events::EventOutcome,
    model::{
        AttachmentMetadata, Connection, Message, MessageRole, NameField, PermissionMode, Session,
        SessionStatus, TitleState, validate_name, validate_reasoning_effort,
    },
};

struct Submission<'a> {
    app: &'a App,
    session_id: &'a str,
    settled: bool,
    completion: Option<TurnCompletion>,
}

struct UserMessage<'a> {
    content: String,
    message_id: Uuid,
    selected_skills: &'a [SkillSelection],
    attachments: &'a [MessageAttachment],
    attachment_bytes: Vec<Vec<u8>>,
    fleet: bool,
}

impl Drop for Submission<'_> {
    fn drop(&mut self) {
        self.app.finish_submission(self.session_id);
        if !self.settled {
            if let Some(completion) = &self.completion {
                completion.fail();
            }
            self.app
                .fail_session(self.session_id, "Sending was canceled. Try again.");
        }
    }
}

impl App {
    fn finish_submission(&self, id: &str) -> bool {
        match self.submissions.finish(id) {
            Ok((preparing, canceled)) => {
                if preparing {
                    self.emit(id, "preparation", json!({"preparing":false}));
                }
                canceled
            }
            Err(error) => {
                tracing::error!(%error, "could not clear pending submission");
                true
            }
        }
    }

    pub(crate) async fn session_gate(&self, id: &str) -> Arc<Mutex<()>> {
        let mut gates = self.session_gates.lock().await;
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(id).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(Mutex::new(()));
        gates.insert(id.to_owned(), Arc::downgrade(&gate));
        gate
    }

    pub(crate) async fn create_session(
        &self,
        project_id: Option<String>,
        provider_id: String,
        model_id: String,
        permission_mode: Option<PermissionMode>,
        reasoning_effort: Option<String>,
    ) -> Result<Session> {
        validate_reasoning_effort(reasoning_effort.as_deref())?;
        let permission_mode = permission_mode.unwrap_or(if project_id.is_some() {
            PermissionMode::AllowAll
        } else {
            PermissionMode::Ask
        });
        if project_id.is_none() && permission_mode != PermissionMode::Ask {
            return Err(Error::ChatPermissionMode);
        }
        let _guard = self.mutation_gate.lock().await;
        let provider = self.store.provider(&provider_id)?;
        let model = provider
            .models
            .iter()
            .find(|model| model.id == model_id)
            .ok_or_else(|| Error::ModelNotConfigured {
                provider_id: provider_id.clone(),
                model_id: model_id.clone(),
            })?;
        model.validate_reasoning_effort(reasoning_effort.as_deref())?;
        if let Some(id) = &project_id {
            self.store.project(id)?;
        }
        let session = Session {
            title_state: TitleState::Pending,
            id: Uuid::new_v4().to_string(),
            title: if project_id.is_some() {
                "New work"
            } else {
                "New chat"
            }
            .into(),
            project_id,
            provider_id,
            model_id,
            permission_mode,
            reasoning_effort,
            created_at: now(),
            updated_at: now(),
            status: SessionStatus::Idle,
            context_usage: None,
        };
        self.store.save_session(&session)?;
        Ok(session)
    }

    pub(crate) async fn set_permission_mode(
        &self,
        id: &str,
        permission_mode: PermissionMode,
    ) -> Result<Session> {
        self.editable_work_session(id)?;
        let gate = self.session_gate(id).await;
        let _session_guard = gate
            .try_lock()
            .map_err(|_| Error::PermissionModeChangeBusy)?;
        let updated = {
            let _mutation_guard = self.mutation_gate.lock().await;
            let session = self.editable_work_session(id)?;
            if session.permission_mode == permission_mode {
                return Ok(session);
            }
            let sessions = self.sessions.lock().await;
            // Invalidate before saving without an await in between. If the request
            // is cancelled during disconnect, the old policy can never be reused.
            if let Some(live) = sessions.get(id) {
                live.completion.fail();
            }
            self.store.update_session(id, |session| {
                session.permission_mode = permission_mode;
                session.updated_at = now();
            })?
        };
        self.disconnect_session(id).await;
        Ok(updated)
    }

    fn editable_work_session(&self, id: &str) -> Result<Session> {
        let session = self.store.session(id)?;
        if session.project_id.is_none() {
            return Err(Error::WorkSessionRequired);
        }
        if session.status == SessionStatus::Running || self.turns.has_pending_permissions(id)? {
            return Err(Error::PermissionModeChangeRunning);
        }
        Ok(session)
    }

    pub(crate) async fn send_message(
        self: &Arc<Self>,
        session_id: &str,
        content: String,
        message_id: Uuid,
        selected_skills: &[SkillSelection],
        attachments: &[MessageAttachment],
        fleet: bool,
    ) -> Result<()> {
        if (content.trim().is_empty() && attachments.is_empty()) || content.len() > 500_000 {
            return Err(Error::InvalidMessage);
        }
        if fleet && self.store.session(session_id)?.project_id.is_none() {
            return Err(Error::ChatFleetMode);
        }
        let attachment_bytes = validate_attachments(attachments)?;
        if !selected_skills.is_empty() && self.store.session(session_id)?.project_id.is_none() {
            return Err(Error::ChatSkillSelection);
        }
        let gate = self.session_gate(session_id).await;
        let _session_guard = gate.try_lock().map_err(|_| Error::ConversationRunning)?;
        // A retried transport request must never submit the same persisted message twice.
        if self
            .store
            .has_message(session_id, &message_id.to_string())?
        {
            return Err(Error::MessageAlreadyRecorded);
        }
        // Do not queue a second prompt behind an active turn. Once idle is visible,
        // wait for its forwarding task to finish before marking the next turn running.
        if self.store.session(session_id)?.status == SessionStatus::Running {
            return Err(Error::ConversationRunning);
        }
        let previous = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .map(|live| live.completion.clone());
        if let Some(completion) = previous {
            completion.wait().await?;
        }
        let (session, first_turn, cancellation) = {
            let _mutation_guard = self.mutation_gate.lock().await;
            let session = self.store.session(session_id)?;
            if session.status == SessionStatus::Running {
                return Err(Error::ConversationRunning);
            }
            let provider = self.store.provider(&session.provider_id)?;
            for attachment in attachments {
                validate_attachment_protocol(
                    &attachment.name,
                    &attachment.mime_type,
                    provider.protocol.into(),
                )?;
            }
            self.titles.mark_interrupted(session_id, false)?;
            let first_turn = !self.store.has_messages(session_id)?;
            let cancellation = self.submissions.begin(session_id)?;
            let session = self
                .store
                .update_session(session_id, |session| {
                    session.status = SessionStatus::Running;
                    session.updated_at = now();
                })
                .inspect_err(|_| {
                    self.finish_submission(session_id);
                })?;
            (session, first_turn, cancellation)
        };
        self.emit(session_id, "status", json!({"status":"running"}));
        let mut submission = Submission {
            app: self,
            session_id,
            settled: false,
            completion: None,
        };
        let result = self
            .submit_turn(
                &session,
                first_turn,
                UserMessage {
                    content,
                    message_id,
                    selected_skills,
                    attachments,
                    attachment_bytes,
                    fleet,
                },
                &mut submission,
                cancellation,
            )
            .await;
        if let Err(error) = result {
            if let Some(completion) = &submission.completion {
                completion.fail();
            }
            self.disconnect_session(session_id).await;
            self.fail_session(session_id, &format!("Sending failed: {error}"));
            submission.settled = true;
            return Err(error);
        }
        submission.settled = true;
        Ok(())
    }

    async fn submit_turn(
        self: &Arc<Self>,
        session: &Session,
        first_turn: bool,
        request: UserMessage<'_>,
        submission: &mut Submission<'_>,
        mut cancellation: watch::Receiver<bool>,
    ) -> Result<()> {
        let initialization = async {
            let handle = self
                .ensure_session(session, !first_turn, submission)
                .await?;
            let prepared = handle
                .prepare(
                    &request.content,
                    request.selected_skills,
                    request.attachments,
                    request.fleet,
                )
                .await?;
            let completion = submission
                .completion
                .as_ref()
                .ok_or(Error::SessionDisconnected)?;
            completion.begin().await?;
            Ok::<_, Error>((handle, prepared))
        };
        let (handle, prepared) = tokio::select! {
            biased;
            _ = cancellation.wait_for(|canceled| *canceled) => return Err(Error::SubmissionCanceled),
            result = initialization => result?,
        };
        // Remove the cancellation slot atomically before committing the message. Stop
        // either cancels preparation or aborts the resulting agent turn, never a later retry.
        if self.finish_submission(&session.id) {
            return Err(Error::SubmissionCanceled);
        }
        let message = Message {
            id: request.message_id.to_string(),
            session_id: session.id.clone(),
            role: MessageRole::User,
            content: request.content,
            selected_skills: request.selected_skills.to_vec(),
            attachments: request
                .attachments
                .iter()
                .zip(&request.attachment_bytes)
                .map(|(attachment, bytes)| AttachmentMetadata {
                    id: Uuid::new_v4().to_string(),
                    name: attachment.name.clone(),
                    mime_type: attachment.mime_type.clone(),
                    size: bytes.len(),
                })
                .collect(),
            created_at: now(),
            sequence: None,
            activity: None,
            reasoning: None,
            agent_id: None,
            agent: None,
            fleet: request.fleet,
            interrupted: false,
        };
        let message = self
            .store
            .append_message_with_attachments(&message, &request.attachment_bytes)?;
        self.store.update_session(&session.id, |current| {
            current.updated_at = now();
            current.status = SessionStatus::Running;
        })?;
        self.emit(&session.id, "message", serde_json::to_value(message)?);
        self.emit(&session.id, "status", json!({"status":"running"}));
        handle.send(prepared).await
    }

    async fn cached_session(
        &self,
        id: &str,
        reasoning_effort: Option<&str>,
    ) -> Option<(AgentHandle, TurnCompletion)> {
        let stale = {
            let mut sessions = self.sessions.lock().await;
            let live = sessions.get(id)?;
            if live.handle.is_connected()
                && !live.event_task.is_finished()
                && !live.completion.has_failed()
                && live.reasoning_effort.as_deref() == reasoning_effort
            {
                return Some((live.handle.clone(), live.completion.clone()));
            }
            sessions.remove(id)
        };
        if let Some(mut live) = stale {
            live.stop_forwarding().await;
            if let Err(error) = live.handle.disconnect().await {
                tracing::warn!(%error, "could not close stale agent session");
            }
        }
        None
    }

    async fn ensure_session(
        self: &Arc<Self>,
        session: &Session,
        resume: bool,
        submission: &mut Submission<'_>,
    ) -> Result<AgentHandle> {
        let project = session
            .project_id
            .as_ref()
            .map(|id| self.store.project(id))
            .transpose()?;
        let options = self.session_options(
            session,
            project.as_ref().map(|project| project.path.clone()),
        )?;
        let cached = self
            .cached_session(&session.id, options.model.reasoning_effort.as_deref())
            .await;
        if let Some((_, completion)) = &cached {
            submission.completion = Some(completion.clone());
        }
        if let Some((AgentHandle::Local(agent), _)) = &cached {
            return Ok(AgentHandle::Local(agent.clone()));
        }
        if let Some((
            AgentHandle::Remote {
                session_id,
                transport,
            },
            _,
        )) = cached
        {
            self.start_remote_agent(&transport, &options, true).await?;
            return Ok(AgentHandle::Remote {
                session_id,
                transport,
            });
        }
        let handle = if let Some(project) = project
            && matches!(project.connection, Connection::Ssh { .. })
        {
            let transport = Arc::new(
                self.remote_session_transport(&session.id, &project.connection)
                    .await?,
            );
            AgentHandle::Remote {
                session_id: session.id.clone(),
                transport,
            }
        } else {
            let runtime = self.ensure_runtime().await?;
            let agent = if resume {
                runtime.resume_session(&session.id, &options).await?
            } else {
                runtime.create_session(options.clone()).await?
            };
            AgentHandle::Local(Arc::new(agent))
        };
        // Subscribe before starting the remote agent so its earliest events are retained.
        let incoming = handle.subscribe();
        if let AgentHandle::Remote { transport, .. } = &handle {
            self.start_remote_agent(transport, &options, resume).await?;
        }
        let completion = TurnCompletion::new();
        submission.completion = Some(completion.clone());
        let event_task = self.forward_events(session.id.clone(), incoming, completion.clone());
        let live = LiveSession {
            handle: handle.clone(),
            event_task,
            completion,
            reasoning_effort: options.model.reasoning_effort,
        };
        self.sessions.lock().await.insert(session.id.clone(), live);
        Ok(handle)
    }

    async fn remote_session_transport(
        &self,
        session_id: &str,
        connection: &Connection,
    ) -> Result<RemoteTransport> {
        let key = serde_json::to_string(connection)?;
        let gate = self
            .remote_environments
            .lock()
            .await
            .entry(key)
            .or_default()
            .clone();
        // Serializing checks avoids duplicate bootstrap work, but waiting for another
        // conversation is not itself evidence that this host needs initialization.
        let _guard = gate.lock().await;
        let client = self.remote_client(connection)?;
        match client.connect_ready().await {
            Ok(transport) => return Ok(transport),
            Err(error) => tracing::debug!(%error, "remote environment must be prepared"),
        }
        self.mark_preparing(session_id)?;
        let transport = client.connect().await?;
        Ok(transport)
    }

    fn mark_preparing(&self, session_id: &str) -> Result<()> {
        self.submissions.prepare(session_id)?;
        self.emit(session_id, "preparation", json!({"preparing":true}));
        Ok(())
    }

    fn forward_events(
        self: &Arc<Self>,
        session_id: String,
        mut incoming: AgentEvents,
        completion: TurnCompletion,
    ) -> JoinHandle<()> {
        let app = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let event = incoming.recv().await;
                let Some(app) = app.upgrade() else { break };
                let keep_forwarding = completion.process_event(|| match event {
                    Ok(event) => app.receive_event(&event),
                    Err(error) => {
                        app.fail_session(&session_id, &format!("{error:#}"));
                        EventOutcome::SessionFailed
                    }
                });
                if !keep_forwarding {
                    break;
                }
            }
        })
    }

    async fn start_remote_agent(
        &self,
        transport: &RemoteTransport,
        options: &SessionOptions,
        resume: bool,
    ) -> Result<()> {
        transport
            .request(
                "start_agent",
                json!({"options":options,"resume":resume,"telemetry":self.store.preferences()?.telemetry}),
            )
            .await?;
        Ok(())
    }

    fn session_options(
        &self,
        session: &Session,
        directory: Option<String>,
    ) -> Result<SessionOptions> {
        let (provider, mut model) = self.model_settings(&session.provider_id, &session.model_id)?;
        if let Some(effort) = &session.reasoning_effort {
            model.reasoning_effort = Some(effort.clone());
        }
        Ok(SessionOptions {
            session_id: Some(session.id.clone()),
            permission_mode: session.permission_mode,
            mode: directory.map_or(SessionMode::Chat, |directory| SessionMode::Work {
                directory: directory.into(),
            }),
            provider,
            model,
            system_prompt: Some("Respond in the user's language. Be precise about what you actually did and verified.".into()),
        })
    }

    pub(crate) async fn abort_session(&self, id: &str) -> Result<()> {
        self.submissions.cancel(id)?;
        let gate = self.session_gate(id).await;
        let _session_guard = gate.lock().await;
        self.store.session(id)?;
        self.titles.mark_interrupted(id, true)?;
        let handle = self
            .sessions
            .lock()
            .await
            .get(id)
            .map(|live| (live.handle.clone(), live.completion.clone()));
        if let Some((handle, completion)) = handle {
            handle.abort().await?;
            completion.wait().await?;
        } else {
            self.store.session(id)?;
        }
        self.set_status(id, SessionStatus::Idle)?;
        self.emit(id, "status", json!({"status":"idle"}));
        Ok(())
    }

    pub(crate) async fn respond_permission(
        &self,
        id: &str,
        request_id: &str,
        approved: bool,
    ) -> Result<()> {
        let handle = self
            .sessions
            .lock()
            .await
            .get(id)
            .map(|live| live.handle.clone())
            .ok_or(Error::PermissionSessionDisconnected)?;
        handle.approve(request_id, approved).await?;
        self.turns.resolve_permission(id, request_id)
    }

    pub(crate) async fn disconnect_session(&self, id: &str) {
        if let Err(error) = self.submissions.cancel(id) {
            tracing::error!(%error, "could not cancel pending submission");
        }
        let live = self.sessions.lock().await.remove(id);
        if let Some(mut live) = live {
            live.stop_forwarding().await;
            if let Err(error) = live.handle.disconnect().await {
                tracing::warn!(%error, "could not disconnect agent session");
            }
        }
        if let Err(error) = self.persist_interrupted(id) {
            tracing::error!(%error, "could not save interrupted output on disconnect");
        }
    }

    pub(crate) async fn delete_session(&self, id: &str) -> Result<()> {
        let _guard = self.mutation_gate.lock().await;
        if self.store.session(id)?.status == SessionStatus::Running {
            return Err(Error::ConversationRunning);
        }
        self.disconnect_session(id).await;
        let mut titles = self.titles.lock()?;
        self.store.delete_session(id)?;
        titles.cancel(id);
        Ok(())
    }

    pub(crate) async fn rename_session(&self, id: &str, title: &str) -> Result<()> {
        validate_name(title, NameField::ConversationName)?;
        let _guard = self.mutation_gate.lock().await;
        let mut titles = self.titles.lock()?;
        self.store.update_session(id, |session| {
            session.title_state = TitleState::Preserved;
            session.title = title.trim().into();
            session.updated_at = now();
        })?;
        titles.cancel(id);
        self.emit(id, "title", json!({"title":title.trim()}));
        Ok(())
    }

    pub(crate) async fn shutdown_sessions(&self) {
        match self.submissions.pending() {
            Ok(ids) => {
                for id in ids {
                    if let Err(error) = self.submissions.cancel(&id) {
                        tracing::error!(%error, "could not cancel pending submission during shutdown");
                    }
                }
            }
            Err(error) => {
                tracing::error!(%error, "could not list pending submissions during shutdown");
            }
        }
        let ids: Vec<String> = self.sessions.lock().await.keys().cloned().collect();
        for id in ids {
            self.disconnect_session(&id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use anyhow::{Context, Result};
    use copilot_runtime::{EventKind, RuntimeEvent};
    use tempfile::TempDir;

    use super::*;
    use crate::{AppOptions, MemoryVault, Request, model::Project};

    fn application() -> Result<(TempDir, Arc<App>)> {
        let directory = TempDir::new()?;
        let app = App::open(AppOptions {
            data_directory: directory.path().into(),
            secrets: Arc::new(MemoryVault::default()),
            remote_artifacts: tandem_remote::ArtifactLocator::default(),
        })?;
        Ok((directory, app))
    }

    async fn session(app: &App) -> Result<Session> {
        let provider = app.save_provider(
            serde_json::from_value(json!({
                "id":"", "name":"Fixture", "protocol":"openai-completions",
                "baseUrl":"http://127.0.0.1:9999/v1", "models":[{"id":"fixture", "name":"Fixture", "contextWindow":200_000}]
            }))?,
            None,
        ).await?;
        Ok(app
            .create_session(
                None,
                provider.id,
                "fixture".into(),
                Some(PermissionMode::Ask),
                None,
            )
            .await?)
    }

    async fn project(app: &App) -> Result<Project> {
        Ok(app
            .create_project(
                "Fixture work".into(),
                app.options.data_directory.to_string_lossy().into_owned(),
                Connection::Local,
            )
            .await?)
    }

    async fn work_session(app: &App) -> Result<Session> {
        let chat = session(app).await?;
        let project = project(app).await?;
        Ok(app
            .create_session(
                Some(project.id),
                chat.provider_id,
                chat.model_id,
                Some(PermissionMode::Ask),
                None,
            )
            .await?)
    }

    #[tokio::test]
    async fn waiting_for_another_host_preparation_can_be_stopped_and_retried() -> Result<()> {
        let (_directory, app) = application()?;
        let work = work_session(&app).await?;
        let mut project = app
            .store
            .project(work.project_id.as_deref().context("missing project")?)?;
        project.connection = Connection::Ssh {
            host: "fixture.invalid".into(),
            port: None,
            user: None,
            identity_file: None,
        };
        app.store.save_project(&project)?;
        let gate = app
            .remote_environments
            .lock()
            .await
            .entry(serde_json::to_string(&project.connection)?)
            .or_default()
            .clone();
        let _guard = gate.lock().await;
        for interruption in ["status", "preparation", "disconnect", "drop"] {
            interrupt_preparation(&app, &work, interruption, false).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn recorded_message_id_is_rejected_without_submitting_again() -> Result<()> {
        let (_directory, app) = application()?;
        let session = session(&app).await?;
        let id = Uuid::new_v4();
        app.store.append_message(&Message {
            id: id.to_string(),
            session_id: session.id.clone(),
            role: MessageRole::User,
            content: "Recorded but delivery may be uncertain".into(),
            selected_skills: vec![],
            attachments: vec![],
            created_at: now(),
            sequence: None,
            activity: None,
            reasoning: None,
            agent_id: None,
            agent: None,
            fleet: false,
            interrupted: false,
        })?;
        assert!(matches!(
            app.send_message(&session.id, "Retry".into(), id, &[], &[], false)
                .await,
            Err(Error::MessageAlreadyRecorded)
        ));
        assert_eq!(app.store.messages(&session.id)?.len(), 1);
        assert_eq!(app.submissions.pending()?, Vec::<String>::new());
        assert!(!app.runtime_status().await.available);
        Ok(())
    }

    #[cfg(unix)]
    fn write_ready_ssh_fixture() -> Result<()> {
        let ssh = std::env::var_os("TANDEM_TEST_SSH_PROGRAM").context("missing SSH fixture")?;
        std::fs::write(
            ssh,
            r#"#!/bin/sh
for argument do command=$argument; done
case "$command" in
  *uname*)
    printf 'probe\n' >> "$TANDEM_TEST_SSH_CALLS"
    printf 'Linux\000x86_64\000tandem-server %s (protocol %s)\n' "$TANDEM_TEST_VERSION" "$TANDEM_TEST_PROTOCOL"
    ;;
  *--install-runtime*)
    printf 'runtime\n' >> "$TANDEM_TEST_SSH_CALLS"
    touch "$TANDEM_TEST_SSH_READY"
    ;;
  *--proxy*)
    printf 'proxy\n' >> "$TANDEM_TEST_SSH_CALLS"
    read -r request
    version="$TANDEM_TEST_VERSION"
    if [ -f "$TANDEM_TEST_SSH_FAIL" ]; then
      rm "$TANDEM_TEST_SSH_FAIL"
      version=stale
    fi
    ready=false
    if [ -f "$TANDEM_TEST_SSH_READY" ]; then ready=true; fi
    printf '{"id":1,"value":{"protocolVersion":%s,"serverVersion":"%s","processId":123,"runtimeReady":%s}}\n' "$TANDEM_TEST_PROTOCOL" "$version" "$ready"
    cat >/dev/null
    ;;
  *) exit 71 ;;
esac
"#,
        )?;
        Ok(())
    }

    #[cfg(unix)]
    async fn check_ready_environment_reuse(app: &App, connection: &Connection) -> Result<()> {
        write_ready_ssh_fixture()?;
        let calls = std::env::var_os("TANDEM_TEST_SSH_CALLS").context("missing calls path")?;
        std::fs::write(&calls, "")?;
        // Clear the earlier failure marker before validating cold and warm connections.
        std::fs::remove_file(
            std::env::var_os("TANDEM_TEST_SSH_FAIL").context("missing failure path")?,
        )?;
        for (id, expected_preparing) in [
            ("first", true),
            ("reuse", false),
            ("new-app", false),
            ("cold-daemon", true),
            ("stale", true),
        ] {
            if id == "new-app" {
                // A fresh desktop process has no cached host state, but the remote
                // daemon can still be healthy and ready for another conversation.
                app.remote_environments.lock().await.clear();
            }
            if id == "cold-daemon" {
                std::fs::remove_file(
                    std::env::var_os("TANDEM_TEST_SSH_READY").context("missing ready path")?,
                )?;
            }
            if id == "stale" {
                std::fs::write(
                    std::env::var_os("TANDEM_TEST_SSH_FAIL").context("missing failure path")?,
                    "stale",
                )?;
            }
            let _cancel = app.submissions.begin(id)?;
            let transport = app.remote_session_transport(id, connection).await?;
            assert_eq!(
                app.submissions.preparing()?.contains(&id.to_owned()),
                expected_preparing,
                "unexpected preparation status for {id}",
            );
            app.finish_submission(id);
            transport.shutdown().await;
        }
        let gate = app
            .remote_environments
            .lock()
            .await
            .get(&serde_json::to_string(connection)?)
            .context("missing host gate")?
            .clone();
        let guard = gate.lock().await;
        let _cancel = app.submissions.begin("concurrent-ready")?;
        let waiting = app.remote_session_transport("concurrent-ready", connection);
        tokio::pin!(waiting);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut waiting)
                .await
                .is_err()
        );
        assert_eq!(app.submissions.preparing()?, Vec::<String>::new());
        drop(guard);
        let transport = waiting.await?;
        assert_eq!(app.submissions.preparing()?, Vec::<String>::new());
        app.finish_submission("concurrent-ready");
        transport.shutdown().await;
        assert_eq!(
            std::fs::read_to_string(calls)?,
            "proxy\nprobe\nruntime\nproxy\nproxy\nproxy\nproxy\nprobe\nruntime\nproxy\nproxy\nprobe\nruntime\nproxy\nproxy\n"
        );
        Ok(())
    }

    #[cfg(unix)]
    async fn check_remote_preparation() -> Result<()> {
        let (_directory, app) = application()?;
        let chat = session(&app).await?;
        let project = Project {
            id: Uuid::new_v4().to_string(),
            name: "Deferred SSH".into(),
            path: "/project".into(),
            connection: Connection::Ssh {
                host: "fixture.invalid".into(),
                port: None,
                user: None,
                identity_file: None,
            },
            created_at: now(),
        };
        app.store.save_project(&project)?;
        let work = app
            .create_session(
                Some(project.id.clone()),
                chat.provider_id,
                chat.model_id,
                None,
                None,
            )
            .await?;
        assert_eq!(app.list_project_skills(&project.id).await?, vec![]);
        let calls = std::path::PathBuf::from(
            std::env::var_os("TANDEM_TEST_SSH_CALLS").context("missing calls path")?,
        );
        assert!(
            !calls.exists(),
            "session creation and skill discovery must not invoke SSH"
        );
        for interruption in ["status", "preparation", "disconnect", "drop"] {
            interrupt_preparation(&app, &work, interruption, true).await?;
        }
        // A failed SSH attempt must release preparation and preserve the unsent message.
        std::fs::write(
            std::env::var_os("TANDEM_TEST_SSH_FAIL").context("missing failure path")?,
            "fail",
        )?;
        assert!(matches!(
            app.send_message(&work.id, "Retry me".into(), Uuid::new_v4(), &[], &[], false)
                .await,
            Err(Error::Remote(_))
        ));
        assert_eq!(
            app.snapshot().await?.preparing_sessions,
            Vec::<String>::new()
        );
        assert!(app.store.messages(&work.id)?.is_empty());
        check_ready_environment_reuse(&app, &project.connection).await?;
        Ok(())
    }

    async fn interrupt_preparation(
        app: &Arc<App>,
        work: &Session,
        interruption: &str,
        expect_preparation: bool,
    ) -> Result<()> {
        let mut events = app.subscribe();
        let sending = tokio::spawn({
            let app = Arc::clone(app);
            let id = work.id.clone();
            async move {
                app.send_message(
                    &id,
                    "Keep this draft".into(),
                    Uuid::new_v4(),
                    &[],
                    &[],
                    false,
                )
                .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let event = events.recv().await?;
                if ((interruption == "status" || !expect_preparation) && event.kind == "status")
                    || (interruption != "status"
                        && event.kind == "preparation"
                        && event.data["preparing"] == true)
                {
                    break;
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        if interruption != "status" && expect_preparation {
            assert_eq!(
                app.snapshot().await?.preparing_sessions,
                std::slice::from_ref(&work.id)
            );
        }
        assert!(app.store.messages(&work.id)?.is_empty());
        assert!(matches!(
            app.send_message(
                &work.id,
                "Duplicate click".into(),
                Uuid::new_v4(),
                &[],
                &[],
                false
            )
            .await,
            Err(Error::ConversationRunning)
        ));
        match interruption {
            "drop" => {
                sending.abort();
                assert!(sending.await.is_err());
            }
            "disconnect" => {
                app.disconnect_session(&work.id).await;
                assert!(matches!(sending.await?, Err(Error::SubmissionCanceled)));
            }
            _ => {
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    app.abort_session(&work.id),
                )
                .await??;
                assert!(matches!(sending.await?, Err(Error::SubmissionCanceled)));
                assert_eq!(app.store.session(&work.id)?.status, SessionStatus::Idle);
            }
        }
        assert_eq!(
            app.snapshot().await?.preparing_sessions,
            Vec::<String>::new()
        );
        assert!(app.store.messages(&work.id)?.is_empty());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn remote_preparation_is_lazy_cancellable_and_retryable() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        const CHILD: &str = "TANDEM_TEST_PREPARATION";
        if std::env::var_os(CHILD).is_some() {
            return tokio::runtime::Runtime::new()?
                .block_on(async { check_remote_preparation().await });
        }
        let directory = TempDir::new()?;
        let ssh = directory.path().join("ssh");
        std::fs::write(
            &ssh,
            r#"#!/bin/sh
printf '%s\n' called >> "$TANDEM_TEST_SSH_CALLS"
for argument do command=$argument; done
case "$command" in *--proxy*) exit 1 ;; esac
if [ -f "$TANDEM_TEST_SSH_FAIL" ]; then exit 1; fi
exec sleep 60
"#,
        )?;
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700))?;
        let path = format!("{}:{}", directory.path().display(), std::env::var("PATH")?);
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "sessions::tests::remote_preparation_is_lazy_cancellable_and_retryable",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("PATH", path)
            .env("TANDEM_TEST_SSH_PROGRAM", &ssh)
            .env("TANDEM_TEST_VERSION", env!("CARGO_PKG_VERSION"))
            .env(
                "TANDEM_TEST_PROTOCOL",
                tandem_remote::PROTOCOL_VERSION.to_string(),
            )
            .env("TANDEM_TEST_SSH_READY", directory.path().join("ready"))
            .env("TANDEM_TEST_SSH_CALLS", directory.path().join("calls"))
            .env("TANDEM_TEST_SSH_FAIL", directory.path().join("fail"))
            .output()?;
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[tokio::test]
    async fn effective_reasoning_effort_uses_the_session_override_or_current_model_default()
    -> Result<()> {
        let (_directory, app) = application()?;
        let session = session(&app).await?;
        let mut provider = app.store.provider(&session.provider_id)?;
        for (default, selected, expected) in [
            (None, None, None),
            (Some("low"), None, Some("low")),
            (Some("medium"), Some("high"), Some("high")),
            (Some("high"), Some("low"), Some("low")),
            (Some("high"), Some("max"), Some("max")),
            (Some("max"), None, Some("max")),
            (None, Some("medium"), Some("medium")),
            (None, None, None),
        ] {
            provider.models[0].reasoning_effort = default.map(str::to_owned);
            app.save_provider(provider.clone(), None).await?;
            let updated = app
                .set_session_reasoning_effort(&session.id, selected.map(str::to_owned))
                .await?;
            for directory in [None, Some("/fixture".into())] {
                let options = app.session_options(&updated, directory)?;
                assert_eq!(options.model.reasoning_effort.as_deref(), expected);
                // SSH uses this exact SessionOptions payload.
                let remote: SessionOptions =
                    serde_json::from_value(serde_json::to_value(options)?)?;
                assert_eq!(remote.model.reasoning_effort.as_deref(), expected);
            }
            assert_eq!(
                app.store.provider(&session.provider_id)?.models[0]
                    .reasoning_effort
                    .as_deref(),
                default
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn create_session_accepts_optional_reasoning_overrides() -> Result<()> {
        let (_directory, app) = application()?;
        let session = session(&app).await?;
        let mut provider = app.store.provider(&session.provider_id)?;
        provider.models[0].reasoning_effort = Some("medium".into());
        app.save_provider(provider, None).await?;
        for effort in [None, Some("low"), Some("medium"), Some("high"), Some("max")] {
            let mut payload = json!({"providerId":session.provider_id,"modelId":session.model_id});
            if let Some(effort) = effort {
                payload["reasoningEffort"] = json!(effort);
            }
            let request: Request =
                serde_json::from_value(json!({"command":"create_session","payload":payload}))?;
            let created: Session = serde_json::from_value(app.dispatch(request).await?)?;
            assert_eq!(created.reasoning_effort.as_deref(), effort);
            assert_eq!(
                app.session_options(&created, None)?
                    .model
                    .reasoning_effort
                    .as_deref(),
                effort.or(Some("medium"))
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn context_usage_tracks_the_latest_work_reading_across_reopen() -> Result<()> {
        for work in [false, true] {
            let (directory, app) = application()?;
            let session = if work {
                work_session(&app).await?
            } else {
                session(&app).await?
            };
            let mut events = app.subscribe();
            for current_tokens in [153_000, 20_000] {
                // Round-trip the same event frame used by the remote server.
                let event: RuntimeEvent = serde_json::from_value(json!({
                    "sessionId":session.id,"kind":"context_usage",
                    "data":{"currentTokens":current_tokens,"tokenLimit":200_000},
                }))?;
                assert!(matches!(app.receive_event(&event), EventOutcome::Continue));
                if work {
                    let emitted = events.try_recv()?;
                    assert_eq!(emitted.kind, "context_usage");
                    assert_eq!(emitted.data, event.data);
                    assert_eq!(
                        app.store.session(&session.id)?.context_usage,
                        Some(copilot_runtime::ContextUsage {
                            current_tokens,
                            token_limit: 200_000,
                        })
                    );
                } else {
                    assert!(events.try_recv().is_err());
                    assert!(app.store.session(&session.id)?.context_usage.is_none());
                }
            }
            let before_invalid = app.store.session(&session.id)?.context_usage;
            for data in [json!({}), json!({"currentTokens":-1,"tokenLimit":200_000})] {
                app.receive_event(&RuntimeEvent {
                    session_id: session.id.clone(),
                    kind: EventKind::ContextUsage,
                    data,
                });
                assert_eq!(
                    app.store.session(&session.id)?.context_usage,
                    before_invalid
                );
                assert!(events.try_recv().is_err());
            }
            let reopened = App::open(AppOptions::local(directory.path().into()))?;
            let snapshot = reopened.snapshot().await?;
            let restored = snapshot
                .sessions
                .iter()
                .find(|item| item.id == session.id)
                .context("session missing from snapshot")?;
            assert_eq!(restored.context_usage, before_invalid);
            assert_eq!(restored.updated_at, session.updated_at);
        }
        Ok(())
    }

    #[tokio::test]
    async fn provider_updates_clear_obsolete_context_capacity() -> Result<()> {
        let (_directory, app) = application()?;
        let session = work_session(&app).await?;
        app.receive_event(&RuntimeEvent {
            session_id: session.id.clone(),
            kind: EventKind::ContextUsage,
            data: json!({"currentTokens":153_000,"tokenLimit":200_000}),
        });
        let mut events = app.subscribe();
        let mut provider = app.store.provider(&session.provider_id)?;
        provider.models[0].context_window = 1_000_000;
        app.save_provider(provider, None).await?;
        assert!(app.store.session(&session.id)?.context_usage.is_none());
        let cleared = events.try_recv()?;
        assert_eq!(cleared.session_id, session.id);
        assert_eq!(cleared.kind, "context_usage");
        assert!(cleared.data.is_null());
        Ok(())
    }

    #[tokio::test]
    async fn context_readings_after_idle_are_kept_until_the_connection_is_invalidated() -> Result<()>
    {
        let (_directory, app) = application()?;
        let session = work_session(&app).await?;
        let completion = TurnCompletion::new();
        completion.begin().await?;
        let mut events = app.subscribe();
        let (outgoing, incoming) = tokio::sync::broadcast::channel(16);
        let frame = |kind, data| json!({"sessionId":session.id,"kind":kind,"data":data});
        outgoing.send(frame("status", json!({"state":"idle"})))?;
        outgoing.send(frame(
            "context_usage",
            json!({"currentTokens":153_000,"tokenLimit":200_000}),
        ))?;
        let event_task = app.forward_events(
            session.id.clone(),
            AgentEvents::Remote {
                session_id: session.id.clone(),
                incoming,
            },
            completion.clone(),
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if events.recv().await?.kind == "context_usage" {
                    return Ok::<_, tokio::sync::broadcast::error::RecvError>(());
                }
            }
        })
        .await??;
        completion.wait().await?;
        assert_eq!(app.store.session(&session.id)?.status, SessionStatus::Idle);
        completion.fail();
        outgoing.send(frame(
            "context_usage",
            json!({"currentTokens":199_000,"tokenLimit":200_000}),
        ))?;
        tokio::time::timeout(std::time::Duration::from_secs(1), event_task).await??;
        assert_eq!(
            app.store
                .session(&session.id)?
                .context_usage
                .map(|usage| usage.current_tokens),
            Some(153_000)
        );
        Ok(())
    }

    #[tokio::test]
    async fn session_commands_default_work_to_full_access_and_keep_chat_restricted() -> Result<()> {
        let (_directory, app) = application()?;
        let chat = session(&app).await?;
        let project = project(&app).await?;
        for project_id in [None, Some(project.id)] {
            for mode in [None, Some("ask"), Some("allow-all")] {
                let mut payload = json!({
                    "projectId":project_id, "providerId":chat.provider_id,
                    "modelId":chat.model_id,
                });
                if let Some(mode) = mode {
                    payload["permissionMode"] = json!(mode);
                }
                let request: Request = serde_json::from_value(json!({
                    "command":"create_session", "payload":payload,
                }))?;
                let result = app.dispatch(request).await;
                if project_id.is_none() && mode == Some("allow-all") {
                    assert!(result.is_err());
                } else {
                    let expected = mode.unwrap_or(if project_id.is_some() {
                        "allow-all"
                    } else {
                        "ask"
                    });
                    let created: Session = serde_json::from_value(result?)?;
                    assert_eq!(serde_json::to_value(created.permission_mode)?, expected);
                    assert_eq!(
                        app.store.session(&created.id)?.permission_mode,
                        created.permission_mode
                    );
                    let options = app.session_options(
                        &created,
                        project_id.as_ref().map(|_| "/fixture".into()),
                    )?;
                    assert_eq!(options.permission_mode, created.permission_mode);
                }
            }
        }
        for mode in [PermissionMode::Ask, PermissionMode::AllowAll] {
            assert!(app.set_permission_mode(&chat.id, mode).await.is_err());
        }
        assert_eq!(
            app.store.session(&chat.id)?.permission_mode,
            PermissionMode::Ask
        );
        Ok(())
    }

    #[tokio::test]
    async fn permission_changes_do_not_wait_for_running_initializing_or_pending_work() -> Result<()>
    {
        for (status, pending_approval, initializing) in [
            (SessionStatus::Running, false, false),
            (SessionStatus::Idle, true, false),
            (SessionStatus::Idle, false, true),
            (SessionStatus::Error, true, false),
            (SessionStatus::Error, false, true),
        ] {
            let (_directory, app) = application()?;
            let work = work_session(&app).await?;
            app.set_status(&work.id, status)?;
            if pending_approval {
                app.turns
                    .permission(&work.id, &json!({"requestId":"pending"}))?;
            }
            let gate = app.session_gate(&work.id).await;
            let _initializing = initializing.then(|| gate.try_lock()).transpose()?;
            // A rejected request must finish even while configuration changes are blocked.
            let _mutation_guard = app.mutation_gate.lock().await;
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                app.set_permission_mode(&work.id, PermissionMode::AllowAll),
            )
            .await?;
            assert!(
                result.is_err(),
                "{status:?}/{pending_approval}/{initializing}"
            );
            let stored = app.store.session(&work.id)?;
            assert_eq!(stored.permission_mode, PermissionMode::Ask);
            assert_eq!(stored.status, status);
        }
        Ok(())
    }

    #[tokio::test]
    async fn failed_work_can_revoke_full_permission_without_losing_history() -> Result<()> {
        let (_directory, app) = application()?;
        let work = work_session(&app).await?;
        app.set_permission_mode(&work.id, PermissionMode::AllowAll)
            .await?;
        app.store.append_message(&Message {
            id: Uuid::new_v4().to_string(),
            session_id: work.id.clone(),
            role: MessageRole::Assistant,
            content: "Partial output before the failure".into(),
            attachments: Vec::new(),
            created_at: now(),
            sequence: None,
            activity: None,
            reasoning: None,
            selected_skills: Vec::new(),
            agent_id: None,
            agent: None,
            fleet: false,
            interrupted: true,
        })?;
        app.set_status(&work.id, SessionStatus::Error)?;
        let history = serde_json::to_value(app.store.messages(&work.id)?)?;
        let updated = app
            .set_permission_mode(&work.id, PermissionMode::Ask)
            .await?;
        assert_eq!(updated.permission_mode, PermissionMode::Ask);
        assert_eq!(updated.status, SessionStatus::Error);
        assert_eq!(
            app.store.session(&work.id)?.permission_mode,
            PermissionMode::Ask
        );
        assert_eq!(
            serde_json::to_value(app.store.messages(&work.id)?)?,
            history
        );
        Ok(())
    }

    #[tokio::test]
    async fn permission_changes_preserve_history_and_reach_resumed_local_and_remote_options()
    -> Result<()> {
        let (directory, mut app) = application()?;
        let work = work_session(&app).await?;
        app.rename_session(&work.id, "Keep this title").await?;
        for (role, content) in [
            (MessageRole::User, "Remember this request"),
            (MessageRole::Assistant, "Remember this answer"),
        ] {
            app.store.append_message(&Message {
                id: Uuid::new_v4().to_string(),
                session_id: work.id.clone(),
                role,
                content: content.into(),
                attachments: Vec::new(),
                created_at: now(),
                sequence: None,
                activity: None,
                reasoning: None,
                selected_skills: Vec::new(),
                agent_id: None,
                agent: None,
                fleet: false,
                interrupted: false,
            })?;
        }
        let history = serde_json::to_value(app.store.messages(&work.id)?)?;
        for mode in [PermissionMode::AllowAll, PermissionMode::Ask] {
            let request: Request = serde_json::from_value(json!({
                "command":"set_permission_mode",
                "payload":{"sessionId":work.id,"permissionMode":mode},
            }))?;
            let updated: Session = serde_json::from_value(app.dispatch(request).await?)?;
            assert_eq!(updated.permission_mode, mode);
            assert_eq!(updated.title, "Keep this title");
            app = App::open(AppOptions {
                data_directory: directory.path().into(),
                secrets: Arc::new(MemoryVault::default()),
                remote_artifacts: tandem_remote::ArtifactLocator::default(),
            })?;
            let stored = app.store.session(&work.id)?;
            assert_eq!(stored.permission_mode, mode);
            assert_eq!(stored.status, SessionStatus::Idle);
            assert!(app.store.has_messages(&work.id)?);
            assert_eq!(
                serde_json::to_value(app.store.messages(&work.id)?)?,
                history
            );
            let project = app
                .store
                .project(stored.project_id.as_deref().context("missing project")?)?;
            let options = app.session_options(&stored, Some(project.path))?;
            assert_eq!(options.session_id.as_deref(), Some(work.id.as_str()));
            assert_eq!(options.permission_mode, mode);
            assert!(matches!(options.mode, SessionMode::Work { .. }));
            // SSH sends this exact SessionOptions object to start_agent.
            let remote = serde_json::to_value(&options)?;
            let restored: SessionOptions = serde_json::from_value(remote)?;
            assert_eq!(restored.permission_mode, mode);
        }
        Ok(())
    }

    #[tokio::test]
    async fn same_session_operations_serialize_without_locking_other_sessions() -> Result<()> {
        let (_directory, app) = application()?;
        let first = session(&app).await?;
        let second = session(&app).await?;
        let gate = app.session_gate(&first.id).await;
        let blocked = gate.lock().await;
        let global = app.mutation_gate.lock().await;
        let waiting = tokio::spawn({
            let app = Arc::clone(&app);
            async move { app.abort_session(&first.id).await }
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            app.abort_session(&second.id),
        )
        .await??;
        assert!(!waiting.is_finished());
        drop(global);
        drop(blocked);
        tokio::time::timeout(std::time::Duration::from_secs(1), waiting).await???;
        Ok(())
    }

    #[tokio::test]
    async fn background_status_updates_cannot_overwrite_renamed_titles() -> Result<()> {
        let (_directory, app) = application()?;
        let session = session(&app).await?;
        std::thread::scope(|scope| -> Result<()> {
            let statuses = scope.spawn(|| -> Result<()> {
                for _ in 0..250 {
                    app.set_status(&session.id, SessionStatus::Running)?;
                    app.set_status(&session.id, SessionStatus::Idle)?;
                }
                Ok(())
            });
            for index in 0..250 {
                app.store.update_session(&session.id, |session| {
                    session.title = format!("Renamed {index}");
                })?;
            }
            statuses
                .join()
                .map_err(|_| anyhow::anyhow!("status worker panicked"))??;
            Ok(())
        })?;
        let stored = app.store.session(&session.id)?;
        assert_eq!(stored.title, "Renamed 249");
        assert_eq!(stored.status, SessionStatus::Idle);
        Ok(())
    }

    #[tokio::test]
    async fn unfinished_output_survives_idle_without_duplicating_completed_messages() -> Result<()>
    {
        for completed in [true, false] {
            let (_directory, app) = application()?;
            let session = session(&app).await?;
            let receive = |kind, data| {
                app.receive_event(&RuntimeEvent {
                    session_id: session.id.clone(),
                    kind,
                    data,
                });
            };
            receive(
                EventKind::Delta,
                json!({"messageId":"message-1","content":"Visible output"}),
            );
            receive(
                EventKind::Permission,
                json!({"requestId":"approval-1","kind":"write","description":"Write file"}),
            );
            let snapshot = app.snapshot().await?;
            assert_eq!(snapshot.active_turns[&session.id].stream, "Visible output");
            assert_eq!(snapshot.active_turns[&session.id].permissions.len(), 1);
            if completed {
                receive(
                    EventKind::Message,
                    json!({"id":"message-1","content":"Visible output"}),
                );
            }
            receive(EventKind::Status, json!({"state":"idle"}));
            receive(EventKind::Status, json!({"state":"idle"}));
            let messages = app.store.messages(&session.id)?;
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].content, "Visible output");
            assert_eq!(messages[0].interrupted, !completed);
            assert!(app.snapshot().await?.active_turns.is_empty());
        }
        Ok(())
    }

    #[tokio::test]
    async fn completion_barrier_drains_output_and_discards_events_after_failure() -> Result<()> {
        for (completed, terminal_kind, terminal_data) in [
            (true, EventKind::Status, json!({"state":"idle"})),
            (false, EventKind::Error, json!({"message":"stopped"})),
            (false, EventKind::Status, json!({"state":"invalid"})),
        ] {
            let (_directory, app) = application()?;
            let session = session(&app).await?;
            let completion = TurnCompletion::new();
            completion.begin().await?;
            let (outgoing, incoming) = tokio::sync::broadcast::channel(16);
            let event = |kind, data| RuntimeEvent {
                session_id: session.id.clone(),
                kind,
                data,
            };
            outgoing.send(event(
                EventKind::Delta,
                json!({"messageId":"message-1","content":"reply"}),
            ))?;
            if completed {
                outgoing.send(event(
                    EventKind::Message,
                    json!({"id":"message-1","content":"reply"}),
                ))?;
            }
            outgoing.send(event(terminal_kind, terminal_data))?;
            if !completed {
                // A trailing idle from a failed SDK turn must not overwrite its error.
                outgoing.send(event(EventKind::Status, json!({"state":"idle"})))?;
            }
            // The terminal event is already queued, but the application has not persisted it.
            assert!(app.store.messages(&session.id)?.is_empty());
            let mut event_task = app.forward_events(
                session.id.clone(),
                AgentEvents::Local(incoming),
                completion.clone(),
            );
            completion.wait().await?;
            let messages = app.store.messages(&session.id)?;
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].content, "reply");
            assert_eq!(messages[0].interrupted, !completed);
            assert!(app.turns.snapshot()?.is_empty());
            if completed {
                assert_eq!(app.store.session(&session.id)?.status, SessionStatus::Idle);
                event_task.abort();
                let _ = (&mut event_task).await;
            } else {
                tokio::time::timeout(std::time::Duration::from_secs(1), event_task).await??;
                assert_eq!(app.store.session(&session.id)?.status, SessionStatus::Error);
                assert!(completion.has_failed());
                assert!(completion.begin().await.is_err());
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_submission_preserves_its_error_when_old_idle_is_queued() -> Result<()> {
        let (_directory, app) = application()?;
        let session = session(&app).await?;
        app.set_status(&session.id, SessionStatus::Running)?;
        let completion = TurnCompletion::new();
        completion.begin().await?;
        assert!(
            completion.process_event(|| app.receive_event(&RuntimeEvent {
                session_id: session.id.clone(),
                kind: EventKind::Delta,
                data: json!({"messageId":"message-1","content":"partial reply"}),
            }))
        );
        let (outgoing, incoming) = tokio::sync::broadcast::channel(4);
        outgoing.send(RuntimeEvent {
            session_id: session.id.clone(),
            kind: EventKind::Status,
            data: json!({"state":"idle"}),
        })?;
        let (started, ready) = tokio::sync::oneshot::channel();
        let sending = tokio::spawn({
            let app = Arc::clone(&app);
            let session_id = session.id.clone();
            let completion = completion.clone();
            async move {
                let _submission = Submission {
                    app: &app,
                    session_id: &session_id,
                    settled: false,
                    completion: Some(completion),
                };
                let _ = started.send(());
                std::future::pending::<()>().await;
            }
        });
        ready.await?;
        sending.abort();
        let _ = sending.await;
        let event_task = app.forward_events(
            session.id.clone(),
            AgentEvents::Local(incoming),
            completion.clone(),
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), event_task).await??;
        assert!(completion.has_failed());
        assert_eq!(app.store.session(&session.id)?.status, SessionStatus::Error);
        let messages = app.store.messages(&session.id)?;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "partial reply");
        assert!(messages[0].interrupted);
        assert!(app.turns.snapshot()?.is_empty());
        Ok(())
    }
}
