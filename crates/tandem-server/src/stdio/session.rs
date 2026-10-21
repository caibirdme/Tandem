use std::{
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use copilot_runtime::{
    AgentSession, ApprovalDecision, EventKind, MessageAttachment, PreparedMessage, Runtime,
    RuntimeEvent, SkillSelection, validate_attachments,
};
use serde_json::{Value, json};
use tandem_protocol::ServerFrame;
use tokio::{
    sync::{Mutex as AsyncMutex, broadcast, watch},
    task::JoinHandle,
    time::timeout,
};

use super::{Connection, required};
use crate::error::{Result, ServerError};

#[derive(Default)]
struct Submission {
    generation: u64,
    prepared: Option<(String, PreparedMessage)>,
}

struct Interrupting<'a>(&'a AtomicBool);

impl Drop for Interrupting<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub(super) struct HostedSession {
    agent: AgentSession,
    // A lease prevents process-wide configuration changes while this session lives.
    _runtime: Arc<Runtime>,
    event_task: Mutex<Option<JoinHandle<()>>>,
    submission: Mutex<Submission>,
    operation: AsyncMutex<()>,
    cancelled: watch::Sender<u64>,
    closed: AtomicBool,
    interrupting: AtomicBool,
}

impl HostedSession {
    pub(super) fn new(
        agent: AgentSession,
        runtime: Arc<Runtime>,
        owner: Arc<Connection>,
        id: String,
    ) -> Self {
        let event_task = tokio::spawn(forward_events(agent.subscribe(), owner, id));
        Self {
            agent,
            _runtime: runtime,
            event_task: Mutex::new(Some(event_task)),
            submission: Mutex::new(Submission::default()),
            operation: AsyncMutex::new(()),
            cancelled: watch::channel(0).0,
            closed: AtomicBool::new(false),
            interrupting: AtomicBool::new(false),
        }
    }

    fn submission(&self) -> MutexGuard<'_, Submission> {
        self.submission
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn is_connected(&self) -> bool {
        !self.closed.load(Ordering::Acquire) && self.agent.is_connected()
    }

    fn invalidate(&self) {
        let mut submission = self.submission();
        submission.generation = submission.generation.wrapping_add(1);
        submission.prepared = None;
        self.cancelled.send_replace(submission.generation);
    }

    async fn cancelled_after(&self, generation: u64) {
        let mut cancelled = self.cancelled.subscribe();
        while *cancelled.borrow_and_update() == generation {
            if cancelled.changed().await.is_err() {
                return;
            }
        }
    }

    pub(super) async fn disconnect(&self) -> Result<()> {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.invalidate();
        }
        let _operation = self.operation.lock().await;
        let event_task = self
            .event_task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(mut task) = event_task {
            task.abort();
            let _ = (&mut task).await;
        }
        // SDK disconnect only detaches. Abort first so loss of an approval owner
        // cannot leave work continuing invisibly in the shared CLI.
        let aborted = self.agent.abort().await.map_err(ServerError::from);
        let detached = timeout(Duration::from_secs(5), self.agent.disconnect())
            .await
            .map_err(|_| ServerError::RequestTimeout)?
            .map_err(ServerError::from);
        aborted.and(detached)
    }

    pub(super) async fn dispatch(&self, command: &str, payload: &Value) -> Result<Value> {
        if !self.is_connected() {
            return Err(ServerError::SessionNotConnected);
        }
        match command {
            "abort_agent" => {
                self.interrupting
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .map_err(|_| ServerError::RequestLimit)?;
                let _interrupting = Interrupting(&self.interrupting);
                self.invalidate();
                // Invalidation cancels preparation and submission before abort is
                // sent. New messages cannot overtake the abort request.
                let _operation = self.operation.lock().await;
                self.agent.abort().await?;
            }
            "approve_agent" => {
                let approved = payload["approved"]
                    .as_bool()
                    .ok_or(ServerError::InvalidBoolean { field: "approved" })?;
                self.agent.approve(
                    required(payload, "requestId")?,
                    if approved {
                        ApprovalDecision::Approve
                    } else {
                        ApprovalDecision::Deny
                    },
                )?;
            }
            "send_agent" | "prepare_agent_message" => {
                self.prepare(command, payload).await?;
            }
            "send_prepared_agent_message" => {
                let _operation = self
                    .operation
                    .try_lock()
                    .map_err(|_| ServerError::RequestLimit)?;
                let (generation, message) = {
                    let mut submission = self.submission();
                    self.check_submission()?;
                    let id = required(payload, "preparedId")?;
                    if submission
                        .prepared
                        .as_ref()
                        .is_none_or(|(prepared_id, _)| prepared_id != id)
                    {
                        return Err(ServerError::PreparedMessageUnavailable);
                    }
                    let (_, prepared) = submission
                        .prepared
                        .take()
                        .ok_or(ServerError::PreparedMessageUnavailable)?;
                    (submission.generation, prepared)
                };
                self.send(message, generation).await?;
            }
            _ => return Err(ServerError::UnsupportedCommand),
        }
        Ok(Value::Null)
    }

    async fn prepare(&self, command: &str, payload: &Value) -> Result<()> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| ServerError::RequestLimit)?;
        let selected = selected_skills(payload)?;
        let attachments = message_attachments(payload)?;
        let content = required(payload, "content")?;
        let fleet = match payload.get("fleet") {
            Some(value) => value
                .as_bool()
                .ok_or(ServerError::InvalidBoolean { field: "fleet" })?,
            None => false,
        };
        let id = if command == "prepare_agent_message" {
            Some(required(payload, "preparedId")?)
        } else {
            None
        };
        let generation = {
            let mut submission = self.submission();
            self.check_submission()?;
            submission.prepared = None;
            submission.generation
        };
        let prepared = tokio::select! {
            biased;
            () = self.cancelled_after(generation) => return Err(ServerError::PreparedMessageUnavailable),
            result = self.agent.prepare_message(content, &selected, &attachments, fleet) => result?,
        };
        if let Some(id) = id {
            let mut submission = self.submission();
            if submission.generation != generation || !self.is_connected() {
                return Err(ServerError::PreparedMessageUnavailable);
            }
            submission.prepared = Some((id.to_owned(), prepared));
            Ok(())
        } else {
            self.send(prepared, generation).await
        }
    }

    fn check_submission(&self) -> Result<()> {
        if !self.is_connected() {
            return Err(ServerError::SessionNotConnected);
        }
        if self.interrupting.load(Ordering::Acquire) {
            return Err(ServerError::RequestLimit);
        }
        Ok(())
    }

    async fn send(&self, prepared: PreparedMessage, generation: u64) -> Result<()> {
        tokio::select! {
            biased;
            () = self.cancelled_after(generation) => Err(ServerError::PreparedMessageUnavailable),
            result = self.agent.send_prepared(prepared) => result.map(|_| ()).map_err(Into::into),
        }
    }
}

impl Drop for HostedSession {
    fn drop(&mut self) {
        if let Some(task) = self
            .event_task
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

pub(super) async fn forward_events(
    mut events: broadcast::Receiver<RuntimeEvent>,
    owner: Arc<Connection>,
    session_id: String,
) {
    loop {
        let (event, failed) = match events.recv().await {
            Ok(event) => (event, false),
            Err(broadcast::error::RecvError::Closed) => break,
            Err(broadcast::error::RecvError::Lagged(dropped)) => {
                let error = ServerError::EventStreamOverflow { dropped }.into_rpc();
                (
                    RuntimeEvent {
                        session_id: session_id.clone(),
                        kind: EventKind::Error,
                        data: json!({"code":error.code,"message":error.message}),
                    },
                    true,
                )
            }
        };
        let Ok(event) = serde_json::to_value(event) else {
            break;
        };
        if owner
            .output
            .send(&ServerFrame::Event { event })
            .await
            .is_err()
        {
            owner.close();
            break;
        }
        if failed {
            owner.close();
            break;
        }
    }
}

fn selected_skills(payload: &Value) -> Result<Vec<SkillSelection>> {
    payload
        .get("selectedSkills")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|source| ServerError::InvalidPayload {
            field: "selectedSkills",
            source: source.into(),
        })
        .map(Option::unwrap_or_default)
}

pub(super) fn message_attachments(payload: &Value) -> Result<Vec<MessageAttachment>> {
    let attachments = payload
        .get("attachments")
        .map(|value| serde_json::from_value::<Vec<MessageAttachment>>(value.clone()))
        .transpose()
        .map_err(|source| ServerError::InvalidPayload {
            field: "attachments",
            source: source.into(),
        })?
        .unwrap_or_default();
    validate_attachments(&attachments).map_err(copilot_runtime::Error::from)?;
    Ok(attachments)
}
