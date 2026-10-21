use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use copilot_runtime::{Runtime, RuntimeOptions, SessionOptions, TelemetrySettings};
use serde_json::{Value, json};
use tokio::{
    sync::{Mutex as AsyncMutex, watch},
    time::timeout,
};

use super::{Connection, required, session::HostedSession};
use crate::error::{Result, ServerError};

struct RuntimeState {
    runtime: Arc<Runtime>,
    telemetry: Option<TelemetrySettings>,
}

struct SessionSlot {
    owner: Arc<Connection>,
    session: AsyncMutex<Option<Arc<HostedSession>>>,
}

pub(crate) struct AgentHost {
    runtime: AsyncMutex<Option<RuntimeState>>,
    sessions: Mutex<HashMap<String, Arc<SessionSlot>>>,
    next_connection: AtomicU64,
    stopping: AtomicBool,
    failed: watch::Sender<bool>,
}

enum TelemetryChoice {
    Existing,
    Configure(Option<TelemetrySettings>),
}

impl AgentHost {
    pub(crate) fn new() -> Self {
        Self {
            runtime: AsyncMutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            next_connection: AtomicU64::new(1),
            stopping: AtomicBool::new(false),
            failed: watch::channel(false).0,
        }
    }

    pub(super) fn next_connection(&self) -> u64 {
        self.next_connection.fetch_add(1, Ordering::Relaxed)
    }

    fn sessions(&self) -> MutexGuard<'_, HashMap<String, Arc<SessionSlot>>> {
        // Registry critical sections never execute SDK or user code.
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) async fn install_runtime(&self) -> Result<()> {
        self.ensure_runtime(TelemetryChoice::Existing)
            .await
            .map(|_| ())
    }

    pub(crate) async fn shutdown(&self) -> Result<()> {
        self.stopping.store(true, Ordering::Release);
        let _ = self.close_connections();
        // Stop the process before cleanup when the service exits. Otherwise one
        // unresponsive SDK could multiply the abort deadline by every session.
        let stopped = if let Some(runtime) = self.runtime.lock().await.take() {
            stop_runtime(&runtime.runtime).await
        } else {
            Ok(())
        };
        self.invalidate_sessions().await;
        stopped
    }

    fn close_connections(&self) -> Vec<Arc<Connection>> {
        let owners: Vec<_> = self
            .sessions()
            .values()
            .map(|slot| slot.owner.clone())
            .collect();
        for owner in &owners {
            owner.close();
        }
        owners
    }

    fn fail(&self) {
        self.stopping.store(true, Ordering::Release);
        self.failed.send_replace(true);
        let _ = self.close_connections();
    }

    pub(crate) async fn failed(&self) {
        let mut failed = self.failed.subscribe();
        while !*failed.borrow_and_update() {
            if failed.changed().await.is_err() {
                return;
            }
        }
    }

    async fn invalidate_sessions(&self) {
        let owners = self.close_connections();
        for owner in owners {
            self.disconnect_owner(owner.id).await;
        }
    }

    pub(super) async fn disconnect_owner(&self, owner: u64) {
        let sessions: Vec<_> = self
            .sessions()
            .iter()
            .filter(|(_, slot)| slot.owner.id == owner)
            .map(|(id, slot)| (id.clone(), slot.clone()))
            .collect();
        for (id, slot) in sessions {
            let _ = self.disconnect_slot(&id, &slot).await;
        }
    }

    async fn disconnect_slot(&self, id: &str, slot: &Arc<SessionSlot>) -> Result<()> {
        let mut session = slot.session.lock().await;
        let result = if let Some(session) = session.as_ref() {
            session.disconnect().await
        } else {
            Ok(())
        };
        if result.is_err() {
            self.fail();
        }
        *session = None;
        let mut sessions = self.sessions();
        if sessions
            .get(id)
            .is_some_and(|current| Arc::ptr_eq(current, slot))
        {
            sessions.remove(id);
        }
        result
    }

    async fn ensure_runtime(&self, choice: TelemetryChoice) -> Result<Arc<Runtime>> {
        let mut state = self.runtime.lock().await;
        if self.stopping.load(Ordering::Acquire) {
            return Err(ServerError::RuntimeUnavailable);
        }
        let desired = match choice {
            TelemetryChoice::Existing => state.as_ref().and_then(|state| state.telemetry.clone()),
            TelemetryChoice::Configure(telemetry) => telemetry,
        };
        if let Some(current) = state.as_ref() {
            let healthy = current.runtime.is_healthy().await;
            let matches = telemetry_matches(current.telemetry.as_ref(), desired.as_ref());
            if healthy && matches {
                return Ok(current.runtime.clone());
            }
            if healthy && Arc::strong_count(&current.runtime) > 1 {
                return Err(ServerError::TelemetryConflict);
            }
            if !healthy {
                // A dead CLI affects every SDK session. Close their transports
                // before replacing it so callers cannot keep stale handles.
                self.invalidate_sessions().await;
                if self.stopping.load(Ordering::Acquire) {
                    return Err(ServerError::RuntimeUnavailable);
                }
            }
            if let Some(previous) = state.take() {
                stop_runtime(&previous.runtime).await?;
            }
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(ServerError::MissingHome)?;
        let runtime = Arc::new(
            Runtime::start(RuntimeOptions {
                state_directory: runtime_directory(&home)?,
                cli_path: None,
                telemetry: desired.clone(),
            })
            .await?,
        );
        *state = Some(RuntimeState {
            runtime: runtime.clone(),
            telemetry: desired,
        });
        Ok(runtime)
    }

    pub(super) async fn dispatch(
        &self,
        connection: &Arc<Connection>,
        command: &str,
        payload: Value,
    ) -> Result<Value> {
        if connection.is_closed() || self.stopping.load(Ordering::Acquire) {
            return Err(ServerError::ConnectionClosed);
        }
        match command {
            "server_info" => Ok(json!({
                "processId": std::process::id(),
                "serverVersion": env!("CARGO_PKG_VERSION"),
                "protocolVersion": tandem_protocol::PROTOCOL_VERSION,
            })),
            "install_runtime" => {
                self.install_runtime().await?;
                Ok(Value::Null)
            }
            "start_agent" => self.start(connection, &payload).await,
            "list_project_skills" => {
                let directory = PathBuf::from(required(&payload, "directory")?);
                let runtime = self.ensure_runtime(telemetry_choice(&payload)?).await?;
                Ok(json!(runtime.list_project_skills(&directory).await?))
            }
            "disconnect_agent" => {
                let id = required(&payload, "sessionId")?;
                let slot = self.slot(connection, id)?;
                if let Some(slot) = slot {
                    self.disconnect_slot(id, &slot).await?;
                }
                Ok(Value::Null)
            }
            "send_agent"
            | "prepare_agent_message"
            | "send_prepared_agent_message"
            | "abort_agent"
            | "approve_agent" => {
                // Validate approval shape before looking up its session.
                if command == "approve_agent" && !payload["approved"].is_boolean() {
                    return Err(ServerError::InvalidBoolean { field: "approved" });
                }
                let slot = self
                    .slot(connection, required(&payload, "sessionId")?)?
                    .ok_or(ServerError::SessionNotConnected)?;
                let hosted = slot
                    .session
                    .lock()
                    .await
                    .clone()
                    .ok_or(ServerError::SessionNotConnected)?;
                hosted.dispatch(command, &payload).await
            }
            _ => Err(ServerError::UnsupportedCommand),
        }
    }

    fn slot(&self, connection: &Connection, id: &str) -> Result<Option<Arc<SessionSlot>>> {
        let slot = self.sessions().get(id).cloned();
        if slot
            .as_ref()
            .is_some_and(|slot| slot.owner.id != connection.id)
        {
            return Err(ServerError::SessionOwned);
        }
        Ok(slot)
    }

    async fn start(&self, connection: &Arc<Connection>, payload: &Value) -> Result<Value> {
        let options: SessionOptions =
            serde_json::from_value(payload["options"].clone()).map_err(|source| {
                ServerError::InvalidPayload {
                    field: "options",
                    source: source.into(),
                }
            })?;
        let id = options
            .session_id
            .as_deref()
            .ok_or(ServerError::RequiredField { field: "sessionId" })?
            .to_owned();
        // Reject an ownership conflict before contacting the shared SDK.
        self.slot(connection, &id)?;
        let runtime = self.ensure_runtime(telemetry_choice(payload)?).await?;
        let slot = {
            let mut sessions = self.sessions();
            if connection.is_closed() || self.stopping.load(Ordering::Acquire) {
                return Err(ServerError::ConnectionClosed);
            }
            let slot = sessions.entry(id.clone()).or_insert_with(|| {
                Arc::new(SessionSlot {
                    owner: connection.clone(),
                    session: AsyncMutex::new(None),
                })
            });
            if slot.owner.id != connection.id {
                return Err(ServerError::SessionOwned);
            }
            slot.clone()
        };
        let mut session = slot.session.lock().await;
        if connection.is_closed()
            || !self
                .sessions()
                .get(&id)
                .is_some_and(|current| Arc::ptr_eq(current, &slot))
        {
            return Err(ServerError::ConnectionClosed);
        }
        if session
            .as_ref()
            .is_some_and(|session| session.is_connected())
        {
            return Ok(json!({"sessionId": id}));
        }
        if let Some(previous) = session.as_ref()
            && let Err(error) = previous.disconnect().await
        {
            self.fail();
            return Err(error);
        }
        *session = None;
        let agent = if payload["resume"].as_bool() == Some(true) {
            runtime.resume_session(&id, &options).await?
        } else {
            runtime.create_session(options).await?
        };
        let hosted = Arc::new(HostedSession::new(
            agent,
            runtime,
            connection.clone(),
            id.clone(),
        ));
        *session = Some(hosted.clone());
        if connection.is_closed() {
            if hosted.disconnect().await.is_err() {
                self.fail();
            }
            return Err(ServerError::ConnectionClosed);
        }
        Ok(json!({"sessionId": id}))
    }
}

struct RuntimeStop<'a> {
    runtime: &'a Runtime,
    armed: bool,
}

impl Drop for RuntimeStop<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.runtime.force_stop();
        }
    }
}

async fn stop_runtime(runtime: &Runtime) -> Result<()> {
    let mut shutdown = RuntimeStop {
        runtime,
        armed: true,
    };
    timeout(Duration::from_secs(10), runtime.stop())
        .await
        .map_err(|_| ServerError::RequestTimeout)??;
    shutdown.armed = false;
    Ok(())
}

fn telemetry_choice(payload: &Value) -> Result<TelemetryChoice> {
    let Some(telemetry) = payload.get("telemetry") else {
        return Ok(TelemetryChoice::Existing);
    };
    if telemetry.is_null() {
        return Ok(TelemetryChoice::Configure(None));
    }
    let enabled = telemetry["enabled"]
        .as_bool()
        .ok_or(ServerError::InvalidBoolean {
            field: "telemetry.enabled",
        })?;
    if !enabled {
        return Ok(TelemetryChoice::Configure(None));
    }
    serde_json::from_value(telemetry.clone())
        .map(|telemetry| TelemetryChoice::Configure(Some(telemetry)))
        .map_err(|source| ServerError::InvalidPayload {
            field: "telemetry",
            source: source.into(),
        })
}

fn telemetry_matches(left: Option<&TelemetrySettings>, right: Option<&TelemetrySettings>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.endpoint == right.endpoint
                && left.protocol == right.protocol
                && left.capture_content == right.capture_content
        }
        _ => false,
    }
}

pub(super) fn runtime_directory(home: &Path) -> Result<PathBuf> {
    let directory = home.join(".local/share/tandem/runtime");
    validate_state_directory(&directory)?;
    Ok(directory)
}

fn validate_state_directory(path: &Path) -> Result<()> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(ServerError::StatePathNotDirectory {
            path: path.to_owned(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                validate_state_directory(parent)?;
            }
            Ok(())
        }
        Err(source) => Err(ServerError::StateDirectoryIo {
            path: path.to_owned(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use anyhow::{Context, Result};
    use tandem_protocol::{RequestEnvelope, ServerFrame, encode_frame, read_frame};
    use tokio::io::{AsyncWriteExt, BufReader, duplex};

    use super::*;
    use crate::stdio::{NORMAL_REQUESTS, run_connection, tests::test_connection};

    #[tokio::test]
    async fn uncertain_cleanup_closes_all_owners_and_wakes_service_shutdown() -> Result<()> {
        let host = AgentHost::new();
        let (first, _frames) = test_connection(1);
        let (second, _other_frames) = test_connection(2);
        for (id, owner) in [("first", first.clone()), ("second", second.clone())] {
            host.sessions().insert(
                id.into(),
                Arc::new(SessionSlot {
                    owner,
                    session: AsyncMutex::new(None),
                }),
            );
        }
        host.fail();
        timeout(Duration::from_secs(1), host.failed()).await?;
        assert!(first.is_closed());
        assert!(second.is_closed());
        let (new, _new_frames) = test_connection(3);
        assert!(matches!(
            host.dispatch(&new, "server_info", Value::Null).await,
            Err(ServerError::ConnectionClosed)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn ownership_survives_cleanup_until_the_old_session_is_released() -> Result<()> {
        let host = Arc::new(AgentHost::new());
        let (owner, _frames) = test_connection(1);
        let (other, _other_frames) = test_connection(2);
        let first = Arc::new(SessionSlot {
            owner: owner.clone(),
            session: AsyncMutex::new(None),
        });
        host.sessions().insert("first".into(), first.clone());
        host.sessions().insert(
            "other".into(),
            Arc::new(SessionSlot {
                owner: other.clone(),
                session: AsyncMutex::new(None),
            }),
        );
        assert!(matches!(
            host.slot(&other, "first"),
            Err(ServerError::SessionOwned)
        ));
        assert!(matches!(
            host.dispatch(&other, "disconnect_agent", json!({"sessionId":"first"}))
                .await,
            Err(ServerError::SessionOwned)
        ));
        let initialization = first.session.lock().await;
        let cleanup_host = host.clone();
        let cleanup = tokio::spawn(async move {
            cleanup_host.disconnect_owner(1).await;
        });
        tokio::task::yield_now().await;
        assert!(matches!(
            host.slot(&other, "first"),
            Err(ServerError::SessionOwned)
        ));
        drop(initialization);
        cleanup.await?;
        assert!(host.slot(&other, "first")?.is_none());
        assert!(host.slot(&other, "other")?.is_some());
        assert!(!other.is_closed());
        Ok(())
    }

    #[tokio::test]
    async fn control_requests_have_capacity_while_normal_requests_wait_for_runtime() -> Result<()> {
        let host = Arc::new(AgentHost::new());
        let _startup = host.runtime.lock().await;
        let (mut client_input, input) = duplex(16 * 1024);
        let (output, client_output) = duplex(16 * 1024);
        let connection = tokio::spawn(run_connection(host.clone(), input, output));
        for id in 0..NORMAL_REQUESTS as u64 {
            client_input
                .write_all(&encode_frame(&RequestEnvelope::new(
                    id,
                    "install_runtime",
                    Value::Null,
                )?)?)
                .await?;
        }
        client_input
            .write_all(&encode_frame(&RequestEnvelope::new(
                100,
                "server_info",
                Value::Null,
            )?)?)
            .await?;
        client_input
            .write_all(&encode_frame(&RequestEnvelope::new(
                101,
                "disconnect_agent",
                json!({"sessionId":"absent"}),
            )?)?)
            .await?;
        let mut responses = BufReader::new(client_output);
        let overloaded = timeout(Duration::from_secs(1), read_frame(&mut responses))
            .await??
            .context("overload response")?;
        let control = timeout(Duration::from_secs(1), read_frame(&mut responses))
            .await??
            .context("control response")?;
        assert!(
            matches!(ServerFrame::decode(&overloaded)?, ServerFrame::Failure { id: 100, error } if error.code == tandem_protocol::RpcErrorCode::RuntimeBusy)
        );
        assert!(matches!(
            ServerFrame::decode(&control)?,
            ServerFrame::Response {
                id: 101,
                value: Value::Null
            }
        ));
        client_input.write_all(b"invalid\n").await?;
        assert!(timeout(Duration::from_secs(1), connection).await??.is_err());
        Ok(())
    }

    #[test]
    fn telemetry_distinguishes_unspecified_from_explicitly_disabled() -> Result<()> {
        assert!(matches!(
            telemetry_choice(&json!({}))?,
            TelemetryChoice::Existing
        ));
        for value in [Value::Null, json!({"enabled":false})] {
            assert!(matches!(
                telemetry_choice(&json!({"telemetry":value}))?,
                TelemetryChoice::Configure(None)
            ));
        }
        for value in [
            json!("private-value"),
            json!({"enabled":"private-value"}),
            json!({"enabled":true}),
        ] {
            let error = telemetry_choice(&json!({"telemetry":value}))
                .err()
                .context("invalid telemetry")?;
            assert!(!format!("{error:?}").contains("private-value"));
        }
        Ok(())
    }
}
