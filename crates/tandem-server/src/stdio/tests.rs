use std::path::PathBuf;

use anyhow::{Context, Result};
use copilot_runtime::{EventKind, MessageAttachment, RuntimeEvent};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, duplex},
    sync::broadcast,
};

use super::{
    host::runtime_directory,
    session::{forward_events, message_attachments},
    *,
};

pub(super) fn test_connection(id: u64) -> (Arc<Connection>, mpsc::Receiver<QueuedFrame>) {
    let (frames, receiver) = mpsc::channel(OUTPUT_CAPACITY);
    (
        Arc::new(Connection {
            id,
            output: Output {
                frames,
                bytes: Arc::new(Semaphore::new(OUTPUT_BYTES as usize)),
            },
            closed: watch::channel(false).0,
        }),
        receiver,
    )
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Result<Self> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        // Exercise SDK state paths containing spaces and non-ASCII characters.
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
fn remote_runtime_uses_tandem_state_without_mutating_history() -> Result<()> {
    for exists in [false, true] {
        let home = TestDirectory::new()?;
        let directory = home.0.join(".local/share/tandem/runtime");
        if exists {
            std::fs::create_dir_all(&directory)?;
            std::fs::write(directory.join("session-history"), b"preserved")?;
        }
        assert_eq!(runtime_directory(&home.0)?, directory);
        assert_eq!(directory.exists(), exists);
        if exists {
            assert_eq!(
                std::fs::read(directory.join("session-history"))?,
                b"preserved"
            );
        }
    }
    Ok(())
}

#[test]
fn remote_runtime_does_not_hide_invalid_or_inaccessible_state_paths() -> Result<()> {
    let home = TestDirectory::new()?;
    let path = home.0.join(".local/share/tandem/runtime");
    std::fs::create_dir_all(path.parent().context("state parent")?)?;
    std::fs::write(&path, b"not a directory")?;
    let error = runtime_directory(&home.0)
        .err()
        .context("invalid state must be reported")?;
    assert!(
        matches!(&error, ServerError::StatePathNotDirectory { path: invalid } if invalid == &path)
    );
    assert_eq!(std::fs::read(&path)?, b"not a directory");
    let home = TestDirectory::new()?;
    std::fs::create_dir_all(home.0.join(".local/share"))?;
    std::fs::write(home.0.join(".local/share/tandem"), b"blocked parent")?;
    match runtime_directory(&home.0) {
        Err(ServerError::StatePathNotDirectory { .. }) => {}
        Err(ServerError::StateDirectoryIo { source, .. }) => {
            assert_eq!(source.kind(), std::io::ErrorKind::NotADirectory);
        }
        result => panic!("expected a non-directory state path error, got {result:?}"),
    }
    Ok(())
}

#[test]
fn attachment_payloads_are_optional_and_validate_binary_contents() -> Result<()> {
    assert_eq!(message_attachments(&json!({}))?, []);
    let valid = MessageAttachment::from_bytes("notes.txt", "text/plain", b"content");
    assert_eq!(
        message_attachments(&json!({"attachments":[valid]}))?,
        [valid]
    );
    for payload in [
        json!({"attachments":null}),
        json!({"attachments":"private-content"}),
        json!({"attachments":[{"name":"notes.txt","mimeType":"text/plain"}]}),
    ] {
        let error = message_attachments(&payload)
            .err()
            .context("invalid attachment shape")?;
        assert_eq!(
            error.into_rpc().code,
            tandem_protocol::RpcErrorCode::InvalidPayload
        );
    }
    let error = message_attachments(&json!({
        "attachments":[{"name":"notes.txt","mimeType":"text/plain","data":"invalid"}]
    }))
    .err()
    .context("invalid attachment encoding")?;
    assert!(matches!(
        error,
        ServerError::Runtime(copilot_runtime::Error::Attachments(
            copilot_runtime::AttachmentError::InvalidBase64 { .. }
        ))
    ));
    Ok(())
}

#[tokio::test]
async fn invalid_command_payloads_have_typed_errors_without_starting_runtime() -> Result<()> {
    type InvalidCommandCase = (&'static str, Value, fn(&ServerError) -> bool);
    let host = AgentHost::new();
    let (connection, _frames) = test_connection(1);
    let cases: [InvalidCommandCase; 4] = [
        ("send_agent", Value::Null, |error| {
            matches!(error, ServerError::RequiredField { field: "sessionId" })
        }),
        ("send_agent", json!({"sessionId":"missing"}), |error| {
            matches!(error, ServerError::SessionNotConnected)
        }),
        (
            "approve_agent",
            json!({"sessionId":"missing","approved":"true"}),
            |error| matches!(error, ServerError::InvalidBoolean { field: "approved" }),
        ),
        (
            "start_agent",
            json!({"options":{"mode":"private-key"}}),
            |error| {
                matches!(
                    error,
                    ServerError::InvalidPayload {
                        field: "options",
                        ..
                    }
                )
            },
        ),
    ];
    for (command, payload, expected) in cases {
        let Err(error) = host.dispatch(&connection, command, payload).await else {
            panic!("invalid command was accepted");
        };
        assert!(expected(&error), "{error:?}");
        assert!(!format!("{error:?}").contains("private-key"));
    }

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
    run_connection(Arc::new(AgentHost::new()), requests.as_slice(), &mut output).await?;
    let mut response = output.as_slice();
    let first = read_frame(&mut response).await?.context("first response")?;
    let second = read_frame(&mut response)
        .await?
        .context("second response")?;
    let mut frames = [ServerFrame::decode(&first)?, ServerFrame::decode(&second)?];
    frames.sort_by_key(|frame| match frame {
        ServerFrame::Response { id, .. } | ServerFrame::Failure { id, .. } => *id,
        ServerFrame::Event { .. } => u64::MAX,
    });
    let [first, second] = frames;
    assert!(matches!(
        first,
        ServerFrame::Failure { id: 0, error } if error.code == tandem_protocol::RpcErrorCode::UnsupportedCommand
    ));
    assert!(matches!(
        second,
        ServerFrame::Response {
            id: 1,
            value: Value::Null
        }
    ));
    assert_eq!(response, b"");
    Ok(())
}

#[tokio::test]
async fn finishing_a_request_preserves_a_partially_received_next_frame() -> Result<()> {
    let host = Arc::new(AgentHost::new());
    let (mut connection, mut frames) = test_connection(1);
    Arc::get_mut(&mut connection)
        .context("unique connection")?
        .output
        .bytes = Arc::new(Semaphore::new(0));
    let (mut client, server) = duplex(1024);
    let reader = tokio::spawn(read_commands(host, connection.clone(), server));
    let first = encode_frame(&RequestEnvelope::new(1, "server_info", Value::Null)?)?;
    let second = encode_frame(&RequestEnvelope::new(
        2,
        "server_info",
        json!({"padding": "split frame"}),
    )?)?;
    let split = second.len() / 2;
    client.write_all(&first).await?;
    client.write_all(&second[..split]).await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    connection.output.bytes.add_permits(OUTPUT_BYTES as usize);
    let first = timeout(Duration::from_secs(1), frames.recv())
        .await?
        .context("first frame")?;
    assert!(matches!(
        ServerFrame::decode(&first.bytes)?,
        ServerFrame::Response { id: 1, .. }
    ));
    tokio::time::sleep(Duration::from_millis(10)).await;
    client.write_all(&second[split..]).await?;
    client.shutdown().await?;
    timeout(Duration::from_secs(1), reader).await???;
    let second = frames.recv().await.context("second frame")?;
    assert!(matches!(
        ServerFrame::decode(&second.bytes)?,
        ServerFrame::Response { id: 2, .. }
    ));
    Ok(())
}

#[tokio::test]
async fn event_forwarders_only_write_to_their_own_connection() -> Result<()> {
    let (owner, mut output) = test_connection(1);
    let (other, mut unrelated) = test_connection(2);
    let (events, incoming) = broadcast::channel(2);
    let forwarding = tokio::spawn(forward_events(incoming, owner, "first".into()));
    events.send(RuntimeEvent {
        session_id: "first".into(),
        kind: EventKind::Delta,
        data: json!({"content":"private"}),
    })?;
    let event = timeout(Duration::from_secs(1), output.recv())
        .await?
        .context("owned event")?;
    assert!(
        matches!(ServerFrame::decode(&event.bytes)?, ServerFrame::Event { event } if event["sessionId"] == "first")
    );
    assert!(matches!(
        unrelated.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    assert!(!other.is_closed());
    drop(events);
    forwarding.await?;
    Ok(())
}

#[tokio::test]
async fn broken_output_stops_the_connection_while_input_remains_open() -> Result<()> {
    let (mut client_input, input) = duplex(1024);
    let (output, client_output) = duplex(1024);
    drop(client_output);
    let connection = tokio::spawn(run_connection(Arc::new(AgentHost::new()), input, output));
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
        let result = timeout(
            Duration::from_secs(1),
            run_connection(Arc::new(AgentHost::new()), input, output),
        )
        .await?;
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
    let (connection, mut frames) = test_connection(1);
    for _ in 0..3 {
        events.send(RuntimeEvent {
            session_id: "session".into(),
            kind: EventKind::Delta,
            data: json!({"content":"part"}),
        })?;
    }
    forward_events(incoming, connection.clone(), "session".into()).await;
    let frame = frames.recv().await.context("overflow event")?;
    assert!(
        matches!(ServerFrame::decode(&frame.bytes)?, ServerFrame::Event { event } if event["sessionId"] == "session" && event["kind"] == "error" && event["data"]["code"] == "runtime_failure")
    );
    assert!(connection.is_closed());
    drop(connection);
    assert!(frames.recv().await.is_none());
    Ok(())
}
