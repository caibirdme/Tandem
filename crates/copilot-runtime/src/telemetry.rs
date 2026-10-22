//! Private SDK file export adapted to `AgentLens` without a local HTTP service.

#[cfg(test)]
mod tests;
mod transform;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use github_copilot_sdk::{Client, OtelExporterType, TelemetryConfig};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{Mutex, mpsc, watch},
    task::JoinHandle,
};

use crate::{AgentLensSettings, ConfigurationError, Result, TelemetryProtocol, TelemetrySettings};

const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;
const MAX_PENDING_BYTES: usize = 64 * 1024 * 1024;
const MAX_PENDING_TRACES: usize = 128;
const MAX_TRACE_SPANS: usize = 4096;
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

/// Export failures contain structured context, never user content or receiver bodies.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    #[error("cannot access the private telemetry spool: {0}")]
    Storage(#[source] std::io::Error),
    #[error("cannot initialize the telemetry HTTP client: {0}")]
    Client(#[source] reqwest::Error),
    #[error("cannot reach the telemetry receiver: {0}")]
    Transport(#[source] reqwest::Error),
    #[error("telemetry receiver returned HTTP {status}; check the endpoint and API key")]
    HttpStatus { status: u16 },
    #[error("telemetry receiver rejected {count} spans; check the AgentLens project configuration")]
    RejectedSpans { count: u64 },
    #[error("the telemetry receiver returned an invalid OTLP response")]
    InvalidResponse,
    #[error("the SDK emitted an invalid telemetry record: {0}")]
    InvalidRecord(#[source] serde_json::Error),
    #[error("a telemetry record exceeded the sixteen MiB export limit")]
    RecordLimit,
    #[error("unfinished telemetry exceeded the bounded export queue")]
    QueueLimit,
    #[error("{count} conversation traces did not finish before the runtime stopped")]
    IncompleteTrace { count: usize },
    #[error("the SDK telemetry span is missing a required identifier or timestamp")]
    InvalidSpan,
    #[error(
        "cannot reclaim the private telemetry spool; use a filesystem supporting sparse files: {0}"
    )]
    SparseFile(#[source] fsys::Error),
    #[error(
        "Windows could not mark the private telemetry spool as sparse; verify filesystem support"
    )]
    SparseFileSetup,
    #[error("the telemetry exporter stopped unexpectedly")]
    TaskStopped,
    #[error("telemetry did not finish exporting within fifteen seconds")]
    FlushTimeout,
}

fn validate(settings: &TelemetrySettings) -> Result<&AgentLensSettings> {
    // Reuse the runtime's endpoint validation before creating an outbound client.
    settings.to_sdk()?;
    let agentlens = settings
        .agentlens
        .as_ref()
        .ok_or(ConfigurationError::EmptyTelemetryKey)?;
    if agentlens.service_name.trim().is_empty()
        || agentlens.service_name.chars().count() > 256
        || agentlens.service_name.chars().any(char::is_control)
    {
        return Err(ConfigurationError::EmptyTelemetryServiceName.into());
    }
    if agentlens.api_key.trim().is_empty() {
        return Err(ConfigurationError::EmptyTelemetryKey.into());
    }
    if settings.protocol != TelemetryProtocol::HttpJson {
        return Err(ConfigurationError::AgentLensRequiresJson.into());
    }
    Ok(agentlens)
}

struct Sender {
    client: reqwest::Client,
    endpoint: String,
    configuration: AgentLensSettings,
}

impl Sender {
    fn new(settings: &TelemetrySettings) -> Result<Self> {
        let configuration = validate(settings)?.clone();
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(HTTP_TIMEOUT)
            .connect_timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|error| TelemetryError::Client(error.without_url()))?;
        Ok(Self {
            client,
            endpoint: format!("{}/v1/traces", settings.endpoint.trim_end_matches('/')),
            configuration,
        })
    }

    async fn send(&self, spans: &[Value]) -> std::result::Result<(), TelemetryError> {
        let body = json!({"resourceSpans":[{
            "resource":{"attributes":[
                {"key":"service.name","value":{"stringValue":self.configuration.service_name}},
                {"key":"tps.tenant.id","value":{"stringValue":self.configuration.api_key}},
            ]},
            "scopeSpans":[{"scope":{"name":"tandem"},"spans":spans}],
        }]});
        let mut response = self
            .client
            .post(&self.endpoint)
            .json(&body)
            .send()
            .await
            .map_err(|error| TelemetryError::Transport(error.without_url()))?;
        if !response.status().is_success() {
            return Err(TelemetryError::HttpStatus {
                status: response.status().as_u16(),
            });
        }
        // OTLP success can be an empty body or JSON. Bound and inspect partial
        // success without retaining an untrusted receiver's diagnostic message.
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| TelemetryError::Transport(error.without_url()))?
        {
            if bytes.len() + chunk.len() > 8192 {
                return Err(TelemetryError::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        if !bytes.iter().all(u8::is_ascii_whitespace) {
            response_result(&bytes)?;
        }
        Ok(())
    }
}

fn response_result(bytes: &[u8]) -> std::result::Result<(), TelemetryError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| TelemetryError::InvalidResponse)?;
    if !value.is_object() {
        return Err(TelemetryError::InvalidResponse);
    }
    if let Some(value) = value.pointer("/partialSuccess/rejectedSpans") {
        let count = value
            .as_u64()
            .or_else(|| value.as_str()?.parse().ok())
            .ok_or(TelemetryError::InvalidResponse)?;
        if count > 0 {
            return Err(TelemetryError::RejectedSpans { count });
        }
    }
    Ok(())
}

pub(crate) struct AgentLensExport {
    path: PathBuf,
    sessions: Arc<RwLock<HashSet<String>>>,
    stop: watch::Sender<bool>,
    task: Mutex<Option<JoinHandle<std::result::Result<(), TelemetryError>>>>,
    capture_content: bool,
    client: Arc<std::sync::Mutex<Option<Client>>>,
    failed: Arc<AtomicBool>,
}

impl AgentLensExport {
    pub(crate) async fn start(settings: &TelemetrySettings, directory: &Path) -> Result<Self> {
        let sender = Sender::new(settings)?;
        let directory = directory.to_path_buf();
        let (temporary, file, path) = tokio::task::spawn_blocking(move || create_spool(&directory))
            .await
            .map_err(|_| TelemetryError::TaskStopped)??;
        let sessions = Arc::new(RwLock::new(HashSet::new()));
        let (stop, receiver) = watch::channel(false);
        let worker_sessions = sessions.clone();
        let client = Arc::new(std::sync::Mutex::new(None::<Client>));
        let worker_client = client.clone();
        let failed = Arc::new(AtomicBool::new(false));
        let worker_failed = failed.clone();
        let task = tokio::spawn(async move {
            let result = export_loop(
                temporary.path().join("spans.jsonl"),
                tokio::fs::File::from_std(file),
                sender,
                worker_sessions,
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
            sessions,
            stop,
            task: Mutex::new(Some(task)),
            capture_content: settings.capture_content,
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

    pub(crate) fn register(&self, session_id: &str) {
        if let Ok(mut sessions) = self.sessions.write() {
            sessions.insert(session_id.into());
        }
    }

    pub(crate) async fn finish(&self) -> Result<()> {
        let _ = self.stop.send(true);
        let Some(task) = self.task.lock().await.take() else {
            return Ok(());
        };
        // An owned cleanup task preserves the deadline when a settings change
        // or application shutdown cancels the caller while export is pending.
        tokio::spawn(finish_task(task))
            .await
            .map_err(|_| TelemetryError::TaskStopped)?
    }

    pub(crate) fn signal_stop(&self) {
        let _ = self.stop.send(true);
    }
}

impl Drop for AgentLensExport {
    fn drop(&mut self) {
        self.signal_stop();
        if let Some(task) = self.task.get_mut().take() {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = finish_task(task).await;
                });
            } else {
                task.abort();
            }
        }
    }
}

async fn finish_task(mut task: JoinHandle<std::result::Result<(), TelemetryError>>) -> Result<()> {
    if let Ok(result) = tokio::time::timeout(Duration::from_secs(15), &mut task).await {
        result.map_err(|_| TelemetryError::TaskStopped)??;
    } else {
        task.abort();
        let _ = task.await;
        return Err(TelemetryError::FlushTimeout.into());
    }
    Ok(())
}

#[derive(Default)]
struct PendingTrace {
    spans: Vec<Value>,
    bytes: usize,
}

struct Queue {
    traces: HashMap<String, PendingTrace>,
    bytes: usize,
}

impl Queue {
    fn new() -> Self {
        Self {
            traces: HashMap::new(),
            bytes: 0,
        }
    }

    fn incomplete(&self, registered: &HashSet<String>) -> usize {
        self.traces
            .values()
            .filter(|trace| {
                trace.spans.iter().any(|span| {
                    span["attributes"]["gen_ai.conversation.id"]
                        .as_str()
                        .is_some_and(|id| registered.contains(id))
                })
            })
            .count()
    }

    fn push(
        &mut self,
        record: Value,
        bytes: usize,
        sessions: &HashSet<String>,
    ) -> std::result::Result<Option<Vec<Value>>, TelemetryError> {
        if record["type"] != "span" {
            return Ok(None);
        }
        if !matches!(
            record["attributes"]["gen_ai.operation.name"].as_str(),
            Some("invoke_agent" | "invoke_workflow" | "chat" | "execute_tool")
        ) {
            return Ok(None);
        }
        let trace_id = record["traceId"]
            .as_str()
            .ok_or(TelemetryError::InvalidSpan)?
            .to_owned();
        let root = matches!(
            record["attributes"]["gen_ai.operation.name"].as_str(),
            Some("invoke_agent" | "invoke_workflow")
        ) && record["parentSpanId"]
            .as_str()
            .is_none_or(|id| id.is_empty() || id == "0000000000000000");
        if !self.traces.contains_key(&trace_id) && self.traces.len() >= MAX_PENDING_TRACES {
            return Err(TelemetryError::QueueLimit);
        }
        let trace = self.traces.entry(trace_id.clone()).or_default();
        if self.bytes.saturating_add(bytes) > MAX_PENDING_BYTES
            || trace.spans.len() >= MAX_TRACE_SPANS
        {
            return Err(TelemetryError::QueueLimit);
        }
        self.bytes += bytes;
        trace.bytes += bytes;
        trace.spans.push(record);
        if !root {
            return Ok(None);
        }
        let trace = self
            .traces
            .remove(&trace_id)
            .ok_or(TelemetryError::InvalidSpan)?;
        self.bytes -= trace.bytes;
        let root = trace.spans.last().ok_or(TelemetryError::InvalidSpan)?;
        let Some(session) = root["attributes"]["gen_ai.conversation.id"]
            .as_str()
            .filter(|id| sessions.contains(*id))
        else {
            return Ok(None);
        };
        transform::trace(&trace.spans, session).map(Some)
    }
}

async fn export_loop(
    path: PathBuf,
    mut file: tokio::fs::File,
    sender: Sender,
    sessions: Arc<RwLock<HashSet<String>>>,
    mut stop: watch::Receiver<bool>,
) -> std::result::Result<(), TelemetryError> {
    let mut queue = Queue::new();
    let mut pending = Vec::new();
    let mut buffer = vec![0_u8; 65_536].into_boxed_slice();
    let mut offset = 0_u64;
    let mut reclaimed = 0_u64;
    let (exports, receiver) = mpsc::channel::<QueuedTrace>(MAX_PENDING_TRACES);
    let outstanding = Arc::new(AtomicUsize::new(0));
    let mut sending = SenderTask(tokio::spawn(send_loop(sender, receiver)));
    loop {
        if sending.0.is_finished() {
            return (&mut sending.0)
                .await
                .map_err(|_| TelemetryError::TaskStopped)?;
        }
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(TelemetryError::Storage)?;
        let count = file
            .read(&mut buffer)
            .await
            .map_err(TelemetryError::Storage)?;
        if count == 0 {
            if *stop.borrow() || stop.has_changed().is_err() {
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
        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            if end > MAX_RECORD_BYTES {
                return Err(TelemetryError::RecordLimit);
            }
            let line: Vec<_> = pending.drain(..=end).collect();
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let record: Value =
                serde_json::from_slice(&line).map_err(TelemetryError::InvalidRecord)?;
            let sessions = sessions
                .read()
                .map_err(|_| TelemetryError::TaskStopped)?
                .clone();
            if let Some(spans) = queue.push(record, line.len(), &sessions)? {
                let trace = queued_trace(spans, &outstanding)?;
                match exports.try_send(trace) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        return Err(TelemetryError::QueueLimit);
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        return (&mut sending.0)
                            .await
                            .map_err(|_| TelemetryError::TaskStopped)?;
                    }
                }
            }
        }
        if pending.len() > MAX_RECORD_BYTES {
            return Err(TelemetryError::RecordLimit);
        }
        scrub(&mut file, &mut buffer[..count], offset).await?;
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
    drop(file);
    if !pending.is_empty() {
        return Err(TelemetryError::InvalidSpan);
    }
    let count = {
        let registered = sessions.read().map_err(|_| TelemetryError::TaskStopped)?;
        queue.incomplete(&registered)
    };
    if count > 0 {
        return Err(TelemetryError::IncompleteTrace { count });
    }
    drop(exports);
    (&mut sending.0)
        .await
        .map_err(|_| TelemetryError::TaskStopped)?
}

struct SenderTask(JoinHandle<std::result::Result<(), TelemetryError>>);

async fn scrub(
    file: &mut tokio::fs::File,
    buffer: &mut [u8],
    offset: u64,
) -> std::result::Result<(), TelemetryError> {
    // Overwrite only bytes already read. The SDK's independent append handle
    // continues after EOF, so concurrent records cannot overlap this region.
    file.seek(std::io::SeekFrom::Start(
        offset - u64::try_from(buffer.len()).map_err(|_| TelemetryError::RecordLimit)?,
    ))
    .await
    .map_err(TelemetryError::Storage)?;
    buffer.fill(0);
    file.write_all(buffer)
        .await
        .map_err(TelemetryError::Storage)?;
    file.flush().await.map_err(TelemetryError::Storage)
}

impl Drop for SenderTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct EncodedSize(usize);

impl std::io::Write for EncodedSize {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(buffer.len());
        Ok(buffer.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct QueuedTrace {
    spans: Vec<Value>,
    bytes: usize,
    outstanding: Arc<AtomicUsize>,
}

fn queued_trace(
    spans: Vec<Value>,
    outstanding: &Arc<AtomicUsize>,
) -> std::result::Result<QueuedTrace, TelemetryError> {
    let mut size = EncodedSize(0);
    serde_json::to_writer(&mut size, &spans).map_err(TelemetryError::InvalidRecord)?;
    if size.0 > 32 * 1024 * 1024 {
        return Err(TelemetryError::QueueLimit);
    }
    outstanding
        .try_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
            queued
                .checked_add(size.0)
                .filter(|next| *next <= MAX_PENDING_BYTES)
        })
        .map_err(|_| TelemetryError::QueueLimit)?;
    Ok(QueuedTrace {
        spans,
        bytes: size.0,
        outstanding: outstanding.clone(),
    })
}

impl Drop for QueuedTrace {
    fn drop(&mut self) {
        self.outstanding.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

async fn send_loop(
    sender: Sender,
    mut receiver: mpsc::Receiver<QueuedTrace>,
) -> std::result::Result<(), TelemetryError> {
    while let Some(trace) = receiver.recv().await {
        let result = sender.send(&trace.spans).await;
        if matches!(
            &result,
            Err(TelemetryError::Transport(_)
                | TelemetryError::HttpStatus {
                    status: 429 | 500..=599
                })
        ) {
            tokio::time::sleep(Duration::from_millis(250)).await;
            sender.send(&trace.spans).await?;
        } else {
            result?;
        }
    }
    Ok(())
}

fn create_spool(
    directory: &Path,
) -> std::result::Result<(tempfile::TempDir, std::fs::File, PathBuf), TelemetryError> {
    let temporary = tempfile::Builder::new()
        .prefix("agentlens-")
        .tempdir_in(directory)
        .map_err(TelemetryError::Storage)?;
    let path = temporary.path().join("spans.jsonl");
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path).map_err(TelemetryError::Storage)?;
    prepare_sparse_file(&path)?;
    // Probe reclamation before content capture rather than retaining a dense file.
    file.set_len(65_536).map_err(TelemetryError::Storage)?;
    fsys::with(fsys::Method::Sync)
        .and_then(|handle| handle.punch_hole(&path, 0, 65_536))
        .map_err(TelemetryError::SparseFile)?;
    file.set_len(0).map_err(TelemetryError::Storage)?;
    Ok((temporary, file, path))
}

#[cfg(windows)]
fn prepare_sparse_file(path: &Path) -> std::result::Result<(), TelemetryError> {
    use std::os::windows::{ffi::OsStrExt, fs::MetadataExt};

    use winapi::um::{winioctl::FSCTL_SET_SPARSE, winnt::FILE_ATTRIBUTE_SPARSE_FILE};
    use winwrap::{
        string::WString,
        um::{
            fileapi::{CreationDisposition, FileAccessRights, ShareModes, create_file_w},
            ioapiset::device_io_control,
        },
    };

    let name = WString::from(path.as_os_str().encode_wide().collect::<Vec<_>>());
    let handle = create_file_w(
        &name,
        FileAccessRights::GENERIC_WRITE,
        ShareModes::FILE_SHARE_READ | ShareModes::FILE_SHARE_WRITE | ShareModes::FILE_SHARE_DELETE,
        None,
        CreationDisposition::OPEN_EXISTING,
        None,
    )
    .map_err(sparse_windows_error)?;
    // FILE_SET_SPARSE_BUFFER contains one BOOLEAN. The synchronous wrapper
    // borrows its buffers and requests ordinary write access to this owned file.
    device_io_control(&handle, FSCTL_SET_SPARSE, &mut [1], &mut [], None)
        .map_err(sparse_windows_error)?;
    if std::fs::metadata(path)
        .map_err(TelemetryError::Storage)?
        .file_attributes()
        & FILE_ATTRIBUTE_SPARSE_FILE
        == 0
    {
        return Err(TelemetryError::SparseFileSetup);
    }
    Ok(())
}

#[cfg(windows)]
fn sparse_windows_error(code: u32) -> TelemetryError {
    TelemetryError::Storage(std::io::Error::from_raw_os_error(i32::from_ne_bytes(
        code.to_ne_bytes(),
    )))
}

#[cfg(not(windows))]
fn prepare_sparse_file(_path: &Path) -> std::result::Result<(), TelemetryError> {
    Ok(())
}
