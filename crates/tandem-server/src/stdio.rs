use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use copilot_runtime::{
    AgentSession, ApprovalDecision, EventKind, Runtime, RuntimeEvent, RuntimeOptions,
    SessionOptions, TelemetrySettings,
};
use serde_json::{Value, json};
use tandem_protocol::{RequestEnvelope, RpcError, ServerFrame, encode_frame, read_frame};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{broadcast, mpsc},
    task::JoinHandle,
    time::timeout,
};

const OUTPUT_CAPACITY: usize = 1024;
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

struct HostedSession {
    agent: AgentSession,
    event_task: JoinHandle<()>,
}

impl HostedSession {
    fn is_connected(&self) -> bool {
        self.agent.is_connected() && !self.event_task.is_finished()
    }

    async fn disconnect(mut self) -> Result<()> {
        self.event_task.abort();
        let _ = (&mut self.event_task).await;
        self.agent.disconnect().await?;
        Ok(())
    }
}

impl Drop for HostedSession {
    fn drop(&mut self) {
        self.event_task.abort();
    }
}

struct AgentHost {
    runtime: Option<Runtime>,
    sessions: HashMap<String, HostedSession>,
    output: mpsc::Sender<ServerFrame>,
}

pub async fn run(install_only: bool) -> Result<()> {
    if install_only {
        let (output, _outgoing) = mpsc::channel(1);
        let mut host = AgentHost::new(output);
        let result = host.ensure_runtime(None).await.map(|_| ());
        return result.and(host.shutdown().await);
    }
    run_connection(tokio::io::stdin(), tokio::io::stdout()).await
}

// Stream ownership stays within one connection, so EOF, invalid input, and a
// broken output pipe all take the same runtime/session cleanup path.
async fn run_connection(
    input: impl AsyncRead + Unpin,
    output: impl AsyncWrite + Unpin,
) -> Result<()> {
    let (sender, outgoing) = mpsc::channel(OUTPUT_CAPACITY);
    let mut host = AgentHost::new(sender);
    let mut writer = Box::pin(write_frames(output, outgoing));
    let (result, writer_finished) = tokio::select! {
        result = read_commands(&mut host, input) => (result, false),
        result = &mut writer => (result, true),
    };
    let cleanup = async {
        let result = host.shutdown().await;
        drop(host);
        result
    };
    if writer_finished {
        return result.and(cleanup.await);
    }
    let (stop_result, drain_result) = tokio::join!(cleanup, async {
        timeout(OUTPUT_DRAIN_TIMEOUT, writer)
            .await
            .context("Remote output did not drain before shutdown")?
    });
    result.and(stop_result).and(drain_result)
}

async fn write_frames(
    mut writer: impl AsyncWrite + Unpin,
    mut outgoing: mpsc::Receiver<ServerFrame>,
) -> Result<()> {
    while let Some(frame) = outgoing.recv().await {
        writer.write_all(&encode_frame(&frame)?).await?;
        writer.flush().await?;
    }
    Ok(())
}

async fn read_commands(host: &mut AgentHost, input: impl AsyncRead + Unpin) -> Result<()> {
    let mut input = BufReader::new(input);
    while let Some(frame) = read_frame(&mut input).await? {
        let request = RequestEnvelope::decode(&frame)?;
        let response = match host.dispatch(&request.command, request.payload).await {
            Ok(value) => ServerFrame::Response {
                id: request.id,
                value,
            },
            Err(error) => ServerFrame::Failure {
                id: request.id,
                error: RpcError {
                    code: "runtime_error".into(),
                    message: format!("{error:#}"),
                },
            },
        };
        host.output.send(response).await?;
    }
    Ok(())
}

async fn forward_events(
    mut events: broadcast::Receiver<RuntimeEvent>,
    output: mpsc::Sender<ServerFrame>,
    session_id: String,
) {
    loop {
        let (event, stream_failed) = match events.recv().await {
            Ok(event) => (event, false),
            Err(broadcast::error::RecvError::Closed) => break,
            Err(broadcast::error::RecvError::Lagged(_)) => (
                RuntimeEvent {
                    session_id: session_id.clone(),
                    kind: EventKind::Error,
                    data: json!({"message":"远端事件流溢出，请重新发送消息恢复会话"}),
                },
                true,
            ),
        };
        // RuntimeEvent contains only known serializable values.
        let Ok(event) = serde_json::to_value(event) else {
            break;
        };
        if output.send(ServerFrame::Event { event }).await.is_err() || stream_failed {
            break;
        }
    }
}

impl AgentHost {
    fn new(output: mpsc::Sender<ServerFrame>) -> Self {
        Self {
            runtime: None,
            sessions: HashMap::new(),
            output,
        }
    }

    async fn shutdown(&mut self) -> Result<()> {
        for (_, session) in self.sessions.drain() {
            let _ = session.disconnect().await;
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.stop().await?;
        }
        Ok(())
    }

    async fn ensure_runtime(&mut self, telemetry: Option<TelemetrySettings>) -> Result<&Runtime> {
        if let Some(runtime) = &self.runtime
            && !runtime.is_healthy().await
        {
            let _ = self.shutdown().await;
        }
        if self.runtime.is_none() {
            let home = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .context("HOME is required on a remote host")?;
            let directory = runtime_directory(&home)?;
            self.runtime = Some(
                Runtime::start(RuntimeOptions {
                    state_directory: directory,
                    cli_path: None,
                    telemetry,
                })
                .await?,
            );
        }
        self.runtime.as_ref().context("Runtime unavailable")
    }

    async fn dispatch(&mut self, command: &str, payload: Value) -> Result<Value> {
        match command {
            "start_agent" => self.start(payload).await,
            "send_agent" => {
                let agent = self.session(&payload)?;
                agent.send(required(&payload, "content")?).await?;
                Ok(Value::Null)
            }
            "abort_agent" => {
                self.session(&payload)?.abort().await?;
                Ok(Value::Null)
            }
            "approve_agent" => {
                let approved = payload["approved"]
                    .as_bool()
                    .context("approved must be a boolean")?;
                self.session(&payload)?.approve(
                    required(&payload, "requestId")?,
                    if approved {
                        ApprovalDecision::Approve
                    } else {
                        ApprovalDecision::Deny
                    },
                )?;
                Ok(Value::Null)
            }
            "disconnect_agent" => {
                if let Some(agent) = self.sessions.remove(required(&payload, "sessionId")?) {
                    agent.disconnect().await?;
                }
                Ok(Value::Null)
            }
            _ => bail!("Unsupported remote command"),
        }
    }

    async fn start(&mut self, payload: Value) -> Result<Value> {
        let options: SessionOptions = serde_json::from_value(payload["options"].clone())?;
        let id = options
            .session_id
            .as_deref()
            .context("sessionId is required")?
            .to_owned();
        if self
            .sessions
            .get(&id)
            .is_some_and(HostedSession::is_connected)
        {
            return Ok(json!({"sessionId":id}));
        }
        if let Some(previous) = self.sessions.remove(&id) {
            let _ = previous.disconnect().await;
        }
        let telemetry = if payload["telemetry"]["enabled"].as_bool() == Some(true) {
            Some(serde_json::from_value(payload["telemetry"].clone())?)
        } else {
            None
        };
        let runtime = self.ensure_runtime(telemetry).await?;
        let agent = if payload["resume"].as_bool() == Some(true) {
            runtime.resume_session(&id, &options).await?
        } else {
            runtime.create_session(options).await?
        };
        let event_task = tokio::spawn(forward_events(
            agent.subscribe(),
            self.output.clone(),
            id.clone(),
        ));
        self.sessions
            .insert(id.clone(), HostedSession { agent, event_task });
        Ok(json!({"sessionId":id}))
    }

    fn session(&self, payload: &Value) -> Result<&AgentSession> {
        self.sessions
            .get(required(payload, "sessionId")?)
            .map(|session| &session.agent)
            .context("Session not connected")
    }
}

fn runtime_directory(home: &Path) -> Result<PathBuf> {
    let current = home.join(".local/share/tandem/runtime");
    if state_directory_exists(&current)? {
        return Ok(current);
    }
    // Reuse the SDK's original state in place so renamed deployments can resume
    // existing remote sessions. Only the executable is deployed under Tandem.
    let legacy = home.join(".local/share/relay/runtime");
    if state_directory_exists(&legacy)? {
        return Ok(legacy);
    }
    Ok(current)
}

fn state_directory_exists(path: &Path) -> Result<bool> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => bail!("SDK state path is not a directory: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Windows can report a missing child when its parent is a regular
            // file. Preserve that failure instead of choosing another history.
            if let Some(parent) = path.parent() {
                state_directory_exists(parent)?;
            }
            Ok(false)
        }
        Err(error) => Err(error)
            .with_context(|| format!("Cannot inspect SDK state directory: {}", path.display())),
    }
}

fn required<'a>(payload: &'a Value, key: &str) -> Result<&'a str> {
    payload[key]
        .as_str()
        .with_context(|| format!("{key} is required"))
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, duplex};

    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Result<Self> {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "tandem-remote-state-{}-{timestamp} 空间",
                std::process::id()
            ));
            std::fs::create_dir(&directory)?;
            Ok(Self(directory))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn remote_runtime_preserves_legacy_sessions_and_prefers_existing_tandem_state() -> Result<()> {
        for (current_exists, legacy_exists, use_legacy) in [
            (false, false, false),
            (false, true, true),
            (true, false, false),
            (true, true, false),
        ] {
            let home = TestDirectory::new()?;
            let current = home.0.join(".local/share/tandem/runtime");
            let legacy = home.0.join(".local/share/relay/runtime");
            for (path, exists) in [(&current, current_exists), (&legacy, legacy_exists)] {
                if exists {
                    std::fs::create_dir_all(path)?;
                    std::fs::write(path.join("session-history"), b"preserved")?;
                }
            }
            let expected = if use_legacy { &legacy } else { &current };
            assert_eq!(runtime_directory(&home.0)?, *expected);
            for (path, exists) in [(&current, current_exists), (&legacy, legacy_exists)] {
                assert_eq!(path.exists(), exists);
                if exists {
                    assert_eq!(std::fs::read(path.join("session-history"))?, b"preserved");
                }
            }
        }
        Ok(())
    }

    #[test]
    fn remote_runtime_does_not_hide_invalid_or_inaccessible_state_paths() -> Result<()> {
        for relative in [".local/share/tandem/runtime", ".local/share/relay/runtime"] {
            let home = TestDirectory::new()?;
            let path = home.0.join(relative);
            std::fs::create_dir_all(path.parent().context("state parent")?)?;
            std::fs::write(&path, b"not a directory")?;
            let error = runtime_directory(&home.0)
                .err()
                .context("invalid state must be reported")?;
            assert!(error.to_string().contains(&path.display().to_string()));
            assert_eq!(std::fs::read(&path)?, b"not a directory");
        }
        let home = TestDirectory::new()?;
        std::fs::create_dir_all(home.0.join(".local/share"))?;
        std::fs::write(home.0.join(".local/share/tandem"), b"blocked parent")?;
        assert!(runtime_directory(&home.0).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn eof_flushes_responses_and_command_errors_do_not_break_the_connection() -> Result<()> {
        let requests = ["unknown_command", "disconnect_agent"]
            .into_iter()
            .enumerate()
            .map(|(id, command)| {
                encode_frame(&RequestEnvelope::new(
                    id as u64,
                    command,
                    json!({"sessionId":"absent"}),
                )?)
            })
            .collect::<Result<Vec<_>, _>>()?
            .concat();
        let mut output = Vec::new();
        run_connection(requests.as_slice(), &mut output).await?;
        let mut response = output.as_slice();
        let first = read_frame(&mut response).await?.context("first response")?;
        assert!(matches!(
            ServerFrame::decode(&first)?,
            ServerFrame::Failure { id: 0, .. }
        ));
        let second = read_frame(&mut response)
            .await?
            .context("second response")?;
        assert!(matches!(
            ServerFrame::decode(&second)?,
            ServerFrame::Response {
                id: 1,
                value: Value::Null
            }
        ));
        assert_eq!(response, b"");
        Ok(())
    }

    #[tokio::test]
    async fn broken_output_stops_the_connection_while_input_remains_open() -> Result<()> {
        let (mut client_input, input) = duplex(1024);
        let (output, client_output) = duplex(1024);
        drop(client_output);
        let connection = tokio::spawn(run_connection(input, output));
        client_input
            .write_all(&encode_frame(&RequestEnvelope::new(
                1,
                "unknown_command",
                Value::Null,
            )?)?)
            .await?;
        assert!(timeout(Duration::from_secs(1), connection).await??.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn malformed_or_truncated_input_does_not_leave_a_writer_waiting() -> Result<()> {
        for input in [
            b"{invalid}\n".as_slice(),
            b"{\"id\":1,\"command\":\"send_agent\"}",
        ] {
            let (output, mut receiver) = duplex(1024);
            let result = timeout(Duration::from_secs(1), run_connection(input, output)).await?;
            assert!(result.is_err());
            let mut bytes = Vec::new();
            receiver.read_to_end(&mut bytes).await?;
            assert_eq!(bytes, [] as [u8; 0]);
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_lagged_event_forwarder_reports_failure_and_releases_output() -> Result<()> {
        let (events, incoming) = broadcast::channel(1);
        let (output, mut frames) = mpsc::channel(2);
        for _ in 0..3 {
            events.send(RuntimeEvent {
                session_id: "session".into(),
                kind: EventKind::Delta,
                data: json!({"content":"part"}),
            })?;
        }
        forward_events(incoming, output, "session".into()).await;
        let frame = frames.recv().await.context("overflow event")?;
        assert!(
            matches!(frame, ServerFrame::Event { event } if event["sessionId"] == "session" && event["kind"] == "error")
        );
        assert!(frames.recv().await.is_none());
        Ok(())
    }
}
