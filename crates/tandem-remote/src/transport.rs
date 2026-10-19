use std::{
    collections::HashMap,
    process::Stdio,
    sync::{
        Arc, Mutex as StdMutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use serde_json::Value;
use tandem_protocol::{RequestEnvelope, ServerFrame, encode_frame, read_frame};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{Mutex, broadcast, mpsc, oneshot, watch},
    task::JoinHandle,
    time::timeout,
};

use crate::{
    DisconnectReason, RemoteError,
    ssh::{STDERR_LIMIT, missing_pipe, safe_diagnostic},
};

const MAX_PENDING_REQUESTS: usize = 128;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
type Pending = Arc<StdMutex<HashMap<u64, oneshot::Sender<Result<Value, RemoteError>>>>>;

struct TransportInner {
    stdin: Mutex<Option<ChildStdin>>,
    pending: Pending,
    events: broadcast::Receiver<Value>,
    next_id: AtomicU64,
    closed: watch::Receiver<Option<Arc<DisconnectReason>>>,
    shutdown: mpsc::Sender<()>,
}

impl Drop for TransportInner {
    fn drop(&mut self) {
        let _ = self.shutdown.try_send(());
    }
}

/// Multiplexed requests and events on one encrypted SSH process.
///
/// Clones share a connection. Dropping the last clone closes SSH and reaps the
/// process; request cancellation removes its response waiter. A disconnected
/// transport is never reused: establish a new connection and resume the session.
#[derive(Clone)]
pub struct RemoteTransport {
    inner: Arc<TransportInner>,
}

impl RemoteTransport {
    pub(crate) fn spawn(mut command: Command) -> Result<Self, RemoteError> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child.stdin.take().ok_or_else(missing_pipe)?;
        let stdout = child.stdout.take().ok_or_else(missing_pipe)?;
        let stderr = child.stderr.take().ok_or_else(missing_pipe)?;
        let pending = Arc::new(StdMutex::new(HashMap::new()));
        let (events, subscription) = broadcast::channel(512);
        let (closed_sender, closed) = watch::channel(None);
        let (shutdown, stop) = mpsc::channel(1);
        let reader = tokio::spawn(read_responses(stdout, Arc::clone(&pending), events));
        let diagnostic = Arc::new(StdMutex::new(Vec::new()));
        let stderr_reader = tokio::spawn(collect_stderr(stderr, Arc::clone(&diagnostic)));
        tokio::spawn(supervise(
            child,
            reader,
            stderr_reader,
            diagnostic,
            Arc::clone(&pending),
            closed_sender,
            stop,
        ));
        Ok(Self {
            inner: Arc::new(TransportInner {
                stdin: Mutex::new(Some(stdin)),
                pending,
                events: subscription,
                next_id: AtomicU64::new(1),
                closed,
                shutdown,
            }),
        })
    }

    /// Subscribe before starting a session to receive its streamed events.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.inner.events.resubscribe()
    }

    /// Send a request without placing its payload in command arguments or diagnostics.
    ///
    /// # Errors
    /// Returns a remote error, disconnect, invalid frame, or a 120-second timeout.
    pub async fn request(&self, command: &str, payload: Value) -> Result<Value, RemoteError> {
        self.request_with_timeout(command, payload, REQUEST_TIMEOUT)
            .await
    }

    async fn request_with_timeout(
        &self,
        command: &str,
        payload: Value,
        deadline: Duration,
    ) -> Result<Value, RemoteError> {
        let id = self
            .inner
            .next_id
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| RemoteError::RequestIdExhausted)?;
        let frame = encode_frame(&RequestEnvelope::new(id, command, payload)?)?;
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = lock(&self.inner.pending);
            if let Some(reason) = self.inner.closed.borrow().as_ref() {
                return Err(RemoteError::Disconnected(reason.clone()));
            }
            if pending.len() >= MAX_PENDING_REQUESTS {
                return Err(RemoteError::TooManyRequests);
            }
            pending.insert(id, sender);
        }
        let mut registration = PendingRegistration {
            id,
            pending: Arc::clone(&self.inner.pending),
            write_in_progress: false,
            shutdown: self.inner.shutdown.clone(),
        };
        let operation = async {
            {
                let mut input = self.inner.stdin.lock().await;
                let writer = input.as_mut().ok_or_else(|| {
                    RemoteError::Disconnected(Arc::new(DisconnectReason::InputClosed))
                })?;
                registration.write_in_progress = true;
                writer.write_all(&frame).await?;
                writer.flush().await?;
                registration.write_in_progress = false;
            }
            receiver.await.map_err(|_| {
                RemoteError::Disconnected(Arc::new(DisconnectReason::ResponseChannelClosed))
            })?
        };
        timeout(deadline, operation)
            .await
            .map_err(|_| RemoteError::Timeout {
                operation: "remote request",
                seconds: deadline.as_secs(),
            })?
    }

    /// Whether the supervisor has observed this SSH connection ending.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.inner.closed.borrow().is_some() || self.inner.events.is_closed()
    }

    /// Close the SSH process, cancel pending requests, and wait briefly for cleanup.
    pub async fn shutdown(&self) {
        let mut closed = self.inner.closed.clone();
        if closed.borrow().is_some() {
            return;
        }
        // EOF lets the server detach sessions, flush telemetry, and stop its CLI.
        let close_input = async {
            drop(self.inner.stdin.lock().await.take());
        };
        if timeout(Duration::from_secs(1), close_input).await.is_ok()
            && timeout(Duration::from_secs(5), closed.changed())
                .await
                .is_ok()
        {
            return;
        }
        let _ = self.inner.shutdown.try_send(());
        let _ = timeout(Duration::from_secs(5), closed.changed()).await;
    }
}

struct PendingRegistration {
    id: u64,
    pending: Pending,
    write_in_progress: bool,
    shutdown: mpsc::Sender<()>,
}

impl Drop for PendingRegistration {
    fn drop(&mut self) {
        lock(&self.pending).remove(&self.id);
        // A canceled partial write cannot be followed by another JSON frame.
        if self.write_in_progress {
            let _ = self.shutdown.try_send(());
        }
    }
}

fn lock<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

async fn supervise(
    mut child: Child,
    mut reader: JoinHandle<Result<(), RemoteError>>,
    mut stderr_reader: JoinHandle<()>,
    diagnostic: Arc<StdMutex<Vec<u8>>>,
    pending: Pending,
    closed: watch::Sender<Option<Arc<DisconnectReason>>>,
    mut stop: mpsc::Receiver<()>,
) {
    enum Termination {
        Process(std::io::Result<std::process::ExitStatus>),
        Reader(Result<Result<(), RemoteError>, tokio::task::JoinError>),
        Closed,
    }
    let (mut termination, reader_finished) = tokio::select! {
        status = child.wait() => (Termination::Process(status), false),
        result = &mut reader => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            (Termination::Reader(result), true)
        }
        _ = stop.recv() => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            (Termination::Closed, false)
        }
    };
    if !reader_finished {
        match timeout(Duration::from_millis(500), &mut reader).await {
            Ok(result @ (Ok(Err(_)) | Err(_))) => termination = Termination::Reader(result),
            Ok(Ok(Ok(()))) => {}
            Err(_) => reader.abort(),
        }
    }
    if timeout(Duration::from_millis(500), &mut stderr_reader)
        .await
        .is_err()
    {
        stderr_reader.abort();
    }
    let diagnostic = safe_diagnostic(&lock(&diagnostic));
    let reason = Arc::new(match termination {
        Termination::Process(Ok(status)) => DisconnectReason::ProcessExited { status, diagnostic },
        Termination::Process(Err(source)) => {
            DisconnectReason::ProcessMonitor { source, diagnostic }
        }
        Termination::Reader(Ok(Err(source))) => {
            DisconnectReason::ResponseFailed { source, diagnostic }
        }
        Termination::Reader(Ok(Ok(()))) => DisconnectReason::OutputClosed { diagnostic },
        Termination::Reader(Err(source)) => DisconnectReason::ReaderStopped { source, diagnostic },
        Termination::Closed => DisconnectReason::Closed,
    });
    let _ = closed.send(Some(Arc::clone(&reason)));
    let requests = std::mem::take(&mut *lock(&pending));
    for sender in requests.into_values() {
        let _ = sender.send(Err(RemoteError::Disconnected(reason.clone())));
    }
}

async fn read_responses(
    stdout: impl AsyncRead + Unpin,
    pending: Pending,
    events: broadcast::Sender<Value>,
) -> Result<(), RemoteError> {
    let mut reader = BufReader::new(stdout);
    while let Some(frame) = read_frame(&mut reader).await? {
        dispatch_frame(ServerFrame::decode(&frame)?, &pending, &events);
    }
    Ok(())
}

fn dispatch_frame(frame: ServerFrame, pending: &Pending, events: &broadcast::Sender<Value>) {
    let (id, result) = match frame {
        ServerFrame::Event { event } => {
            let _ = events.send(event);
            return;
        }
        ServerFrame::Response { id, value } => (id, Ok(value)),
        ServerFrame::Failure { id, error } => (id, Err(RemoteError::Remote(error))),
    };
    if let Some(sender) = lock(pending).remove(&id) {
        let _ = sender.send(result);
    }
}

async fn collect_stderr(mut stderr: impl AsyncRead + Unpin, diagnostic: Arc<StdMutex<Vec<u8>>>) {
    let mut buffer = [0_u8; 2048];
    loop {
        let count = match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(count) => count,
        };
        let mut tail = lock(&diagnostic);
        let discard = tail
            .len()
            .saturating_add(count)
            .saturating_sub(STDERR_LIMIT);
        tail.drain(..discard);
        tail.extend_from_slice(&buffer[..count]);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn dispatches_out_of_order_results_and_events_without_losing_nulls() {
        let pending: Pending = Arc::new(StdMutex::new(HashMap::new()));
        let (events, mut subscription) = broadcast::channel(8);
        let (first_tx, first_rx) = oneshot::channel();
        let (second_tx, second_rx) = oneshot::channel();
        lock(&pending).insert(1, first_tx);
        lock(&pending).insert(2, second_tx);
        for frame in [
            json!({"id":2,"value":null}),
            json!({"event":{"kind":"delta"}}),
            json!({"id":1,"value":"done"}),
        ] {
            let frame = ServerFrame::decode(frame.to_string().as_bytes())
                .unwrap_or_else(|error| panic!("decode failed: {error}"));
            dispatch_frame(frame, &pending, &events);
        }
        assert!(matches!(second_rx.await, Ok(Ok(Value::Null))));
        assert!(matches!(first_rx.await, Ok(Ok(value)) if value == "done"));
        assert_eq!(
            subscription.recv().await.ok(),
            Some(json!({"kind":"delta"}))
        );
        assert!(lock(&pending).is_empty());
    }

    #[tokio::test]
    async fn operation_failure_preserves_its_code_and_leaves_other_requests_usable() {
        let pending: Pending = Arc::new(StdMutex::new(HashMap::new()));
        let (events, _) = broadcast::channel(1);
        let (failed_tx, failed_rx) = oneshot::channel();
        let (success_tx, success_rx) = oneshot::channel();
        lock(&pending).insert(1, failed_tx);
        lock(&pending).insert(2, success_tx);
        let failure = ServerFrame::decode(
            br#"{"id":1,"error":{"code":"approval_not_found","message":"approval expired"}}"#,
        )
        .unwrap_or_else(|error| panic!("decode failed: {error}"));
        dispatch_frame(failure, &pending, &events);
        dispatch_frame(
            ServerFrame::Response {
                id: 2,
                value: Value::Null,
            },
            &pending,
            &events,
        );
        assert!(
            matches!(failed_rx.await, Ok(Err(RemoteError::Remote(error))) if error.code == crate::RpcErrorCode::ApprovalNotFound)
        );
        assert!(matches!(success_rx.await, Ok(Ok(Value::Null))));
        assert!(lock(&pending).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn malformed_responses_preserve_protocol_failure_when_closing_requests() {
        let mut command = Command::new("sh");
        command.args(["-c", "read request; printf '%s\\n' '[]'"]);
        let transport =
            RemoteTransport::spawn(command).unwrap_or_else(|error| panic!("spawn failed: {error}"));
        let result = transport
            .request_with_timeout("ping", Value::Null, Duration::from_secs(5))
            .await;
        assert!(
            matches!(result, Err(RemoteError::Disconnected(reason)) if matches!(reason.as_ref(), DisconnectReason::ResponseFailed { source: RemoteError::Protocol(tandem_protocol::ProtocolError::ExpectedObject), .. }))
        );
        transport.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn closes_pending_request_when_child_exits_without_response() {
        let mut command = Command::new("sh");
        command.args(["-c", "read request; exit 7"]);
        let transport =
            RemoteTransport::spawn(command).unwrap_or_else(|error| panic!("spawn failed: {error}"));
        let result = transport
            .request_with_timeout("ping", Value::Null, Duration::from_secs(5))
            .await;
        assert!(matches!(result, Err(RemoteError::Disconnected(_))));
        transport.shutdown().await;
        assert!(transport.is_closed());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timed_out_request_releases_its_pending_registration() {
        let mut command = Command::new("sh");
        command.args(["-c", "while read request; do :; done"]);
        let transport =
            RemoteTransport::spawn(command).unwrap_or_else(|error| panic!("spawn failed: {error}"));
        let result = transport
            .request_with_timeout("ping", Value::Null, Duration::from_millis(25))
            .await;
        assert!(matches!(result, Err(RemoteError::Timeout { .. })));
        assert!(lock(&transport.inner.pending).is_empty());
        transport.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn drains_final_response_and_closes_event_subscription_on_exit() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            r#"read request; printf '%s\n' '{"id":1,"value":null}'"#,
        ]);
        let transport =
            RemoteTransport::spawn(command).unwrap_or_else(|error| panic!("spawn failed: {error}"));
        let mut events = transport.subscribe();
        let result = transport
            .request_with_timeout("ping", Value::Null, Duration::from_secs(5))
            .await;
        assert!(matches!(result, Ok(Value::Null)));
        assert!(matches!(
            timeout(Duration::from_secs(5), events.recv()).await,
            Ok(Err(broadcast::error::RecvError::Closed))
        ));
        transport.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn canceling_partial_write_closes_transport_before_another_frame() {
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 30"]);
        let transport =
            RemoteTransport::spawn(command).unwrap_or_else(|error| panic!("spawn failed: {error}"));
        let result = transport
            .request_with_timeout(
                "send_agent",
                Value::String("x".repeat(2 * 1024 * 1024)),
                Duration::from_millis(25),
            )
            .await;
        assert!(matches!(result, Err(RemoteError::Timeout { .. })));
        transport.shutdown().await;
        assert!(transport.is_closed());
        assert!(lock(&transport.inner.pending).is_empty());
    }
}
