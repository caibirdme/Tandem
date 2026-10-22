//! OTLP/gRPC transport for the SDK's private native telemetry file exporter.

#[cfg(test)]
mod tests;
mod transform;

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use github_copilot_sdk::{Client, OtelExporterType, TelemetryConfig};
use opentelemetry_proto::tonic::collector::{
    metrics::v1::{ExportMetricsServiceRequest, metrics_service_client::MetricsServiceClient},
    trace::v1::{ExportTraceServiceRequest, trace_service_client::TraceServiceClient},
};
use prost::Message;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWrite},
    sync::{Mutex, mpsc, watch},
    task::JoinHandle,
};
use tonic::{
    Code,
    transport::{Channel, ClientTlsConfig, Endpoint},
};

use super::{
    MAX_PENDING_BYTES, MAX_PENDING_TRACES, MAX_RECORD_BYTES, POLL_INTERVAL, TelemetryError,
};
use crate::{Result, TelemetrySettings};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BATCH_RECORDS: usize = 256;

/// Structured gRPC failures exclude collector messages and telemetry contents.
#[derive(Debug, thiserror::Error)]
pub enum GrpcError {
    #[error(
        "cannot initialize the telemetry gRPC connection; check the endpoint and TLS configuration"
    )]
    Connection(#[source] tonic::transport::Error),
    #[error(
        "the telemetry collector returned gRPC {code}; check its endpoint, access policy, and OTLP configuration"
    )]
    Status { code: Code },
    #[error("the telemetry collector rejected {count} {signal}; check the collector configuration")]
    Rejected { signal: &'static str, count: i64 },
    #[error("the SDK emitted an invalid gRPC telemetry record ({field})")]
    InvalidRecord { field: &'static str },
}

impl GrpcError {
    /// Whether retrying requires correcting collector configuration or access.
    #[must_use]
    pub const fn is_invalid_input(&self) -> bool {
        matches!(
            self,
            Self::Rejected { .. }
                | Self::Status {
                    code: Code::InvalidArgument
                        | Code::Unauthenticated
                        | Code::PermissionDenied
                        | Code::Unimplemented
                        | Code::NotFound
                        | Code::FailedPrecondition,
                }
        )
    }

    fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Status {
                code: Code::Unavailable | Code::DeadlineExceeded | Code::ResourceExhausted,
            }
        )
    }
}

struct Sender {
    traces: TraceServiceClient<Channel>,
    metrics: MetricsServiceClient<Channel>,
}

impl Sender {
    fn new(settings: &TelemetrySettings) -> std::result::Result<Self, GrpcError> {
        let mut endpoint = Endpoint::from_shared(settings.endpoint.clone())
            .map_err(GrpcError::Connection)?
            .connect_timeout(REQUEST_TIMEOUT)
            .timeout(REQUEST_TIMEOUT);
        if endpoint.uri().scheme_str() == Some("https") {
            endpoint = endpoint
                .tls_config(ClientTlsConfig::new().with_native_roots())
                .map_err(GrpcError::Connection)?;
        }
        let channel = endpoint.connect_lazy();
        Ok(Self {
            traces: TraceServiceClient::new(channel.clone())
                .max_encoding_message_size(MAX_PENDING_BYTES)
                .max_decoding_message_size(8192),
            metrics: MetricsServiceClient::new(channel)
                .max_encoding_message_size(MAX_PENDING_BYTES)
                .max_decoding_message_size(8192),
        })
    }

    async fn send(&mut self, batch: &Batch) -> std::result::Result<(), GrpcError> {
        if !batch.traces.resource_spans.is_empty() {
            for attempt in 0..2 {
                let result = self
                    .traces
                    .export(batch.traces.clone())
                    .await
                    .map_err(|status| GrpcError::Status {
                        code: status.code(),
                    });
                match result {
                    Err(error) if attempt == 0 && error.retryable() => {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                    result => {
                        let response = result?.into_inner();
                        if let Some(partial) = response.partial_success
                            && partial.rejected_spans != 0
                        {
                            return Err(GrpcError::Rejected {
                                signal: "spans",
                                count: partial.rejected_spans,
                            });
                        }
                        break;
                    }
                }
            }
        }
        if !batch.metrics.resource_metrics.is_empty() {
            for attempt in 0..2 {
                let result = self
                    .metrics
                    .export(batch.metrics.clone())
                    .await
                    .map_err(|status| GrpcError::Status {
                        code: status.code(),
                    });
                match result {
                    Err(error) if attempt == 0 && error.retryable() => {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                    result => {
                        let response = result?.into_inner();
                        if let Some(partial) = response.partial_success
                            && partial.rejected_data_points != 0
                        {
                            return Err(GrpcError::Rejected {
                                signal: "metric data points",
                                count: partial.rejected_data_points,
                            });
                        }
                        break;
                    }
                }
            }
        }
        Ok(())
    }
}

pub(crate) struct GrpcExport {
    path: PathBuf,
    capture_content: bool,
    stop: watch::Sender<bool>,
    task: Mutex<Option<JoinHandle<std::result::Result<(), TelemetryError>>>>,
    client: Arc<std::sync::Mutex<Option<Client>>>,
    failed: Arc<AtomicBool>,
}

impl GrpcExport {
    pub(crate) async fn start(settings: &TelemetrySettings, directory: &Path) -> Result<Self> {
        settings.to_sdk()?;
        let sender = Sender::new(settings).map_err(TelemetryError::from)?;
        let directory = directory.to_path_buf();
        let (temporary, file, path) =
            tokio::task::spawn_blocking(move || super::create_spool(&directory, "grpc-"))
                .await
                .map_err(|_| TelemetryError::TaskStopped)??;
        let (stop, receiver) = watch::channel(false);
        let worker_path = path.clone();
        let client = Arc::new(std::sync::Mutex::new(None::<Client>));
        let worker_client = client.clone();
        let failed = Arc::new(AtomicBool::new(false));
        let worker_failed = failed.clone();
        let task = tokio::spawn(async move {
            let result = export_loop(
                worker_path,
                tokio::fs::File::from_std(file),
                sender,
                receiver,
            )
            .await;
            if result.is_err() {
                worker_failed.store(true, Ordering::Release);
                if let Ok(client) = worker_client.lock()
                    && let Some(client) = client.as_ref()
                {
                    client.force_stop();
                }
            }
            let cleanup = temporary.close().map_err(TelemetryError::Storage);
            result?;
            cleanup?;
            Ok(())
        });
        Ok(Self {
            path,
            capture_content: settings.capture_content,
            stop,
            task: Mutex::new(Some(task)),
            client,
            failed,
        })
    }

    pub(crate) fn configuration(&self) -> TelemetryConfig {
        TelemetryConfig::new()
            .with_exporter_type(OtelExporterType::File)
            .with_file_path(&self.path)
            .with_source_name("tandem")
            .with_capture_content(self.capture_content)
    }

    pub(crate) fn attach_client(&self, client: Client) {
        if let Ok(mut target) = self.client.lock() {
            if self.failed.load(Ordering::Acquire) {
                client.force_stop();
            }
            *target = Some(client);
        }
    }

    pub(crate) fn signal_stop(&self) {
        let _ = self.stop.send(true);
    }

    pub(crate) async fn finish(&self) -> Result<()> {
        self.signal_stop();
        let Some(task) = self.task.lock().await.take() else {
            return Ok(());
        };
        tokio::spawn(super::finish_task(task))
            .await
            .map_err(|_| TelemetryError::TaskStopped)?
    }
}

impl Drop for GrpcExport {
    fn drop(&mut self) {
        self.signal_stop();
        if let Some(task) = self.task.get_mut().take() {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = super::finish_task(task).await;
                });
            } else {
                task.abort();
            }
        }
    }
}

#[derive(Default)]
struct Batch {
    traces: ExportTraceServiceRequest,
    metrics: ExportMetricsServiceRequest,
    records: usize,
}

impl Batch {
    fn push(&mut self, record: serde_json::Value) -> std::result::Result<(), GrpcError> {
        match record["type"].as_str() {
            Some("span") => self.traces.resource_spans.push(transform::span(record)?),
            Some("metric") => self
                .metrics
                .resource_metrics
                .push(transform::metric(&record)?),
            _ => return Err(GrpcError::InvalidRecord { field: "type" }),
        }
        self.records += 1;
        Ok(())
    }

    fn encoded_len(&self) -> usize {
        self.traces
            .encoded_len()
            .saturating_add(self.metrics.encoded_len())
    }
}

struct QueuedBatch {
    batch: Batch,
    bytes: usize,
    outstanding: Arc<AtomicUsize>,
}

impl Drop for QueuedBatch {
    fn drop(&mut self) {
        self.outstanding.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

fn enqueue(
    batch: &mut Batch,
    exports: &mpsc::Sender<QueuedBatch>,
    outstanding: &Arc<AtomicUsize>,
) -> std::result::Result<(), TelemetryError> {
    if batch.records == 0 {
        return Ok(());
    }
    let bytes = batch.encoded_len();
    outstanding
        .try_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
            queued
                .checked_add(bytes)
                .filter(|next| *next <= MAX_PENDING_BYTES)
        })
        .map_err(|_| TelemetryError::QueueLimit)?;
    let queued = QueuedBatch {
        batch: std::mem::take(batch),
        bytes,
        outstanding: outstanding.clone(),
    };
    exports.try_send(queued).map_err(|error| match error {
        mpsc::error::TrySendError::Full(_) => TelemetryError::QueueLimit,
        mpsc::error::TrySendError::Closed(_) => TelemetryError::TaskStopped,
    })
}

async fn enqueue_or_error(
    batch: &mut Batch,
    exports: &mpsc::Sender<QueuedBatch>,
    outstanding: &Arc<AtomicUsize>,
    sending: &mut super::SenderTask,
) -> std::result::Result<(), TelemetryError> {
    if let Err(error) = enqueue(batch, exports, outstanding) {
        if matches!(error, TelemetryError::TaskStopped) {
            // A closed queue means the sender has completed. Preserve its typed
            // collector failure for the local and remote error boundaries.
            return (&mut sending.0)
                .await
                .map_err(|_| TelemetryError::TaskStopped)?;
        }
        return Err(error);
    }
    Ok(())
}

async fn export_loop<F>(
    path: PathBuf,
    mut file: F,
    mut sender: Sender,
    mut stop: watch::Receiver<bool>,
) -> std::result::Result<(), TelemetryError>
where
    F: AsyncRead + AsyncSeek + AsyncWrite + Unpin,
{
    let (exports, mut receiver) = mpsc::channel::<QueuedBatch>(MAX_PENDING_TRACES);
    let mut sending = super::SenderTask(tokio::spawn(async move {
        while let Some(queued) = receiver.recv().await {
            sender.send(&queued.batch).await?;
        }
        Ok(())
    }));
    let outstanding = Arc::new(AtomicUsize::new(0));
    let mut buffer = vec![0_u8; 65_536].into_boxed_slice();
    let mut pending = Vec::new();
    let mut offset = 0_u64;
    let mut reclaimed = 0_u64;
    loop {
        if sending.0.is_finished() {
            return (&mut sending.0)
                .await
                .map_err(|_| TelemetryError::TaskStopped)?;
        }
        // An EOF may complete before the producer appends its final records.
        // Only a read begun after stop can confirm the spool is drained.
        let stopping = *stop.borrow() || stop.has_changed().is_err();
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(TelemetryError::Storage)?;
        let count = file
            .read(&mut buffer)
            .await
            .map_err(TelemetryError::Storage)?;
        if count == 0 {
            if stopping {
                break;
            }
            tokio::select! {
                () = tokio::time::sleep(POLL_INTERVAL) => {},
                _ = stop.changed() => {},
            }
            continue;
        }
        offset += u64::try_from(count).map_err(|_| TelemetryError::RecordLimit)?;
        pending.extend_from_slice(&buffer[..count]);
        let mut batch = Batch::default();
        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            if end > MAX_RECORD_BYTES {
                return Err(TelemetryError::RecordLimit);
            }
            let line: Vec<_> = pending.drain(..=end).collect();
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let record = serde_json::from_slice(&line)
                .map_err(|_| GrpcError::InvalidRecord { field: "JSON" })?;
            batch.push(record)?;
            if batch.records >= MAX_BATCH_RECORDS || batch.encoded_len() >= MAX_RECORD_BYTES {
                enqueue_or_error(&mut batch, &exports, &outstanding, &mut sending).await?;
            }
        }
        enqueue_or_error(&mut batch, &exports, &outstanding, &mut sending).await?;
        if pending.len() > MAX_RECORD_BYTES {
            return Err(TelemetryError::RecordLimit);
        }
        super::scrub(&mut file, &mut buffer[..count], offset).await?;
        let end = offset / 65_536 * 65_536;
        if end > reclaimed {
            let path = path.clone();
            tokio::task::spawn_blocking(move || {
                fsys::with(fsys::Method::Sync).and_then(|handle| handle.punch_hole(&path, 0, end))
            })
            .await
            .map_err(|_| TelemetryError::TaskStopped)?
            .map_err(TelemetryError::SparseFile)?;
            reclaimed = end;
        }
    }
    if !pending.is_empty() {
        return Err(GrpcError::InvalidRecord {
            field: "incomplete JSON line",
        }
        .into());
    }
    drop(file);
    drop(exports);
    (&mut sending.0)
        .await
        .map_err(|_| TelemetryError::TaskStopped)?
}
