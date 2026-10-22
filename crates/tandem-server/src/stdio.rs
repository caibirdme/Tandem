mod host;
mod reviews;
mod session;
#[cfg(test)]
mod tests;

use std::{collections::HashSet, sync::Arc, time::Duration};

pub(crate) use host::AgentHost;
use serde_json::Value;
use tandem_protocol::{RequestEnvelope, ServerFrame, encode_frame, read_frame};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch},
    task::JoinSet,
    time::timeout,
};

use crate::error::{Result, ServerError};

const OUTPUT_CAPACITY: usize = 128;
const OUTPUT_BYTES: u32 = 64 * 1024 * 1024;
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
const NORMAL_REQUESTS: usize = 16;
const CONTROL_REQUESTS: usize = 8;

struct QueuedFrame {
    bytes: Vec<u8>,
    _permit: OwnedSemaphorePermit,
}

#[derive(Clone)]
struct Output {
    frames: mpsc::Sender<QueuedFrame>,
    bytes: Arc<Semaphore>,
}

impl Output {
    async fn send(&self, frame: &ServerFrame) -> Result<()> {
        let bytes = encode_frame(frame)?;
        let count = u32::try_from(bytes.len()).map_err(|_| ServerError::OutputClosed)?;
        timeout(OUTPUT_DRAIN_TIMEOUT, async {
            let permit = self
                .bytes
                .clone()
                .acquire_many_owned(count)
                .await
                .map_err(|_| ServerError::OutputClosed)?;
            self.frames
                .send(QueuedFrame {
                    bytes,
                    _permit: permit,
                })
                .await
                .map_err(|_| ServerError::OutputClosed)
        })
        .await
        .map_err(ServerError::OutputDrainTimeout)?
    }
}

struct Connection {
    id: u64,
    output: Output,
    closed: watch::Sender<bool>,
}

impl Connection {
    fn close(&self) {
        self.closed.send_replace(true);
    }

    fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    async fn cancelled(&self) {
        let mut closed = self.closed.subscribe();
        while !*closed.borrow_and_update() {
            if closed.changed().await.is_err() {
                return;
            }
        }
    }
}

pub async fn run(install_only: bool) -> Result<()> {
    let host = Arc::new(AgentHost::new());
    let result = if install_only {
        host.install_runtime().await
    } else {
        run_connection(host.clone(), tokio::io::stdin(), tokio::io::stdout()).await
    };
    result.and(host.shutdown().await)
}

/// Each transport owns its requests and sessions; the daemon owns the runtime.
pub(crate) async fn run_connection(
    host: Arc<AgentHost>,
    input: impl AsyncRead + Unpin,
    output: impl AsyncWrite + Unpin,
) -> Result<()> {
    let (frames, outgoing) = mpsc::channel(OUTPUT_CAPACITY);
    let connection = Arc::new(Connection {
        id: host.next_connection(),
        output: Output {
            frames,
            bytes: Arc::new(Semaphore::new(OUTPUT_BYTES as usize)),
        },
        closed: watch::channel(false).0,
    });
    let mut writer = Box::pin(write_frames(output, outgoing));
    let (result, writer_finished) = tokio::select! {
        result = read_commands(host.clone(), connection.clone(), input) => (result, false),
        result = &mut writer => (result, true),
        () = connection.cancelled() => (Err(ServerError::ConnectionClosed), false),
        () = host.failed() => (Err(ServerError::RuntimeUnavailable), false),
    };
    connection.close();
    let cleanup = async {
        host.disconnect_owner(connection.id).await;
        drop(connection);
    };
    if writer_finished {
        cleanup.await;
        return result;
    }
    tokio::pin!(cleanup);
    tokio::select! {
        () = &mut cleanup => {}
        drained = &mut writer => {
            cleanup.await;
            return result.and(drained);
        }
    }
    let drained = timeout(OUTPUT_DRAIN_TIMEOUT, writer)
        .await
        .map_err(ServerError::OutputDrainTimeout)?;
    result.and(drained)
}

async fn write_frames(
    mut writer: impl AsyncWrite + Unpin,
    mut outgoing: mpsc::Receiver<QueuedFrame>,
) -> Result<()> {
    while let Some(frame) = outgoing.recv().await {
        timeout(OUTPUT_DRAIN_TIMEOUT, async {
            writer.write_all(&frame.bytes).await?;
            writer.flush().await
        })
        .await
        .map_err(ServerError::OutputDrainTimeout)??;
    }
    Ok(())
}

fn is_control(command: &str) -> bool {
    matches!(
        command,
        "abort_agent" | "approve_agent" | "disconnect_agent"
    )
}

async fn read_commands(
    host: Arc<AgentHost>,
    connection: Arc<Connection>,
    input: impl AsyncRead + Unpin,
) -> Result<()> {
    let normal = Arc::new(Semaphore::new(NORMAL_REQUESTS));
    let control = Arc::new(Semaphore::new(CONTROL_REQUESTS));
    let mut tasks = JoinSet::new();
    let mut pending = HashSet::new();
    let mut input = BufReader::new(input);
    loop {
        let next_frame = read_frame(&mut input);
        tokio::pin!(next_frame);
        let frame = loop {
            tokio::select! {
                result = tasks.join_next(), if !tasks.is_empty() => {
                    let (id, result) = result.ok_or(ServerError::RuntimeUnavailable)?
                        .map_err(ServerError::RequestTask)?;
                    pending.remove(&id);
                    result?;
                }
                frame = &mut next_frame => break frame?,
            }
        };
        let Some(frame) = frame else { break };
        let request = RequestEnvelope::decode(&frame)?;
        if !pending.insert(request.id) {
            return Err(ServerError::RequestLimit);
        }
        let capacity = if is_control(&request.command) {
            &control
        } else {
            &normal
        };
        let Ok(permit) = capacity.clone().try_acquire_owned() else {
            pending.remove(&request.id);
            connection
                .output
                .send(&ServerFrame::Failure {
                    id: request.id,
                    error: ServerError::RequestLimit.into_rpc(),
                })
                .await?;
            continue;
        };
        let host = host.clone();
        let connection = connection.clone();
        tasks.spawn(async move {
            let _permit = permit;
            let result = timeout(
                REQUEST_TIMEOUT,
                host.dispatch(&connection, &request.command, request.payload),
            )
            .await
            .unwrap_or(Err(ServerError::RequestTimeout));
            let response = match result {
                Ok(value) => ServerFrame::Response {
                    id: request.id,
                    value,
                },
                Err(error) => ServerFrame::Failure {
                    id: request.id,
                    error: error.into_rpc(),
                },
            };
            (request.id, connection.output.send(&response).await)
        });
    }
    let drained = timeout(OUTPUT_DRAIN_TIMEOUT, async {
        while let Some(result) = tasks.join_next().await {
            let (_, result) = result.map_err(ServerError::RequestTask)?;
            result?;
        }
        Ok(())
    })
    .await;
    if let Ok(result) = drained {
        result
    } else {
        tasks.shutdown().await;
        Err(ServerError::RequestTimeout)
    }
}

fn required<'a>(payload: &'a Value, key: &'static str) -> Result<&'a str> {
    payload[key]
        .as_str()
        .ok_or(ServerError::RequiredField { field: key })
}
