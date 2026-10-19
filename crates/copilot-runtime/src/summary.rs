//! One-shot text inference without application history or agent capabilities.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use github_copilot_sdk::{
    Client, MessageOptions, SessionConfig, SessionId, SystemMessageConfig, permission,
    session::Session,
};
use tokio::{sync::oneshot, time::timeout};

use crate::{ModelSettings, ProviderSettings, Result, SummaryError};

const CREATE_TIMEOUT: Duration = Duration::from_secs(30);
const INFERENCE_TIMEOUT: Duration = Duration::from_secs(60);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) async fn summarize(
    client: Client,
    directory: PathBuf,
    provider: ProviderSettings,
    model: ModelSettings,
    prompt: String,
) -> Result<String> {
    let id = temporary_id();
    let config = configuration(&provider, &model, directory, id.clone())?;
    let (mut reply, result) = oneshot::channel();
    // The caller owns only the receiver. Dropping its future must never drop a
    // session between creation and deletion or interrupt the cleanup RPCs.
    let _task = tokio::spawn(async move {
        let result = run(&client, config, &id, prompt, &mut reply).await;
        let _ = reply.send(result);
    });
    result.await.map_err(|_| SummaryError::TaskStopped)?
}

fn temporary_id() -> SessionId {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("tandem-title-{}-{timestamp}-{sequence}", std::process::id()).into()
}

fn configuration(
    provider: &ProviderSettings,
    model: &ModelSettings,
    directory: PathBuf,
    id: SessionId,
) -> Result<SessionConfig> {
    let mut config = SessionConfig::default();
    config.session_id = Some(id);
    config.model = Some(model.id.clone());
    config.reasoning_effort.clone_from(&model.reasoning_effort);
    config.provider = Some(provider.to_sdk(model)?);
    config.model_capabilities = model.to_sdk_capabilities()?;
    config.streaming = Some(false);
    // Some([]) is an explicit empty allowlist in the pinned SDK wire format.
    // Also exclude every tool namespace so future defaults cannot broaden it.
    config.available_tools = Some(Vec::new());
    config.tools = Some(Vec::new());
    config.excluded_tools = Some(
        ["builtin:*", "mcp:*", "custom:*"]
            .map(str::to_owned)
            .to_vec(),
    );
    config.system_message = Some(
        SystemMessageConfig::new()
            .with_mode("replace")
            .with_content("Follow the user's text transformation instructions. Return only the requested text. You have no tools or access to files, commands, or external resources."),
    );
    config.working_directory = Some(directory);
    config.enable_config_discovery = Some(false);
    config.enable_file_hooks = Some(false);
    config.enable_skills = Some(false);
    config.enable_host_git_operations = Some(false);
    config.enable_on_demand_instruction_discovery = Some(false);
    config.enable_session_telemetry = Some(false);
    config.enable_session_store = Some(false);
    config.enable_github_cache_store = Some(false);
    config.skip_custom_instructions = Some(true);
    config.skip_embedding_retrieval = Some(true);
    config.request_extensions = Some(false);
    config.request_canvas_renderer = Some(false);
    Ok(config.with_permission_handler(permission::deny_all()))
}

async fn run(
    client: &Client,
    config: SessionConfig,
    id: &SessionId,
    prompt: String,
    reply: &mut oneshot::Sender<Result<String>>,
) -> Result<String> {
    // Do not cancel an in-flight create merely because the caller left: wait
    // for its session handle, then clean it up without sending another prompt.
    let created = timeout(CREATE_TIMEOUT, client.create_session(config)).await;
    let (session, result) = match created {
        Ok(Ok(session)) => {
            let result = tokio::select! {
                biased;
                () = reply.closed() => Err(SummaryError::Cancelled.into()),
                response = session.send_and_wait(
                    MessageOptions::new(prompt).with_wait_timeout(INFERENCE_TIMEOUT),
                ) => match response {
                    Ok(Some(event)) => event.data.get("content")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .ok_or_else(|| SummaryError::NoText.into()),
                    Ok(None) => Err(SummaryError::NoText.into()),
                    // Provider errors may echo request data. Keep background
                    // title failures safe for the application's error logs.
                    Err(error) => Err(match error.kind() {
                        github_copilot_sdk::ErrorKind::Session(
                            github_copilot_sdk::SessionErrorKind::Timeout(_),
                        ) => SummaryError::InferenceTimeout,
                        _ => SummaryError::InferenceFailed,
                    }.into()),
                },
            };
            (Some(session), result)
        }
        Ok(Err(_)) => (None, Err(SummaryError::CreationFailed.into())),
        Err(_) => (None, Err(SummaryError::CreationTimeout.into())),
    };
    cleanup(client, id, session.as_ref(), result.is_err()).await?;
    result
}

async fn cleanup(
    client: &Client,
    id: &SessionId,
    session: Option<&Session>,
    abort: bool,
) -> Result<()> {
    if let Some(session) = session {
        if abort {
            let _ = timeout(CLEANUP_TIMEOUT, session.abort()).await;
        }
        let _ = timeout(CLEANUP_TIMEOUT, session.disconnect()).await;
    }
    // Disconnect preserves disk history. Delete even when create, abort, or
    // detach failed; a known caller-supplied ID makes that attempt possible.
    timeout(CLEANUP_TIMEOUT, client.delete_session(id))
        .await
        .map_err(|_| SummaryError::DeletionTimeout)?
        .map_err(|_| SummaryError::DeletionFailed.into())
}

#[cfg(test)]
mod tests;
