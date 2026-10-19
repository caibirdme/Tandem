use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use copilot_runtime::{AgentSession, ApprovalDecision, RuntimeEvent};
use serde_json::{Value, json};
use tandem_remote::RemoteTransport;
use tokio::{
    sync::{broadcast, watch},
    task::JoinHandle,
};

use crate::events::EventOutcome;

/// Application operations are independent of the agent's execution location.
#[derive(Clone)]
pub(crate) enum AgentHandle {
    Local(Arc<AgentSession>),
    Remote {
        session_id: String,
        transport: Arc<RemoteTransport>,
    },
}

impl AgentHandle {
    pub fn subscribe(&self) -> AgentEvents {
        match self {
            Self::Local(agent) => AgentEvents::Local(agent.subscribe()),
            Self::Remote { transport, .. } => AgentEvents::Remote(transport.subscribe()),
        }
    }

    pub fn is_connected(&self) -> bool {
        match self {
            Self::Local(agent) => agent.is_connected(),
            Self::Remote { transport, .. } => !transport.is_closed(),
        }
    }

    pub async fn send(&self, content: &str) -> Result<()> {
        match self {
            Self::Local(agent) => {
                agent.send(content).await?;
            }
            Self::Remote {
                session_id,
                transport,
            } => {
                transport
                    .request(
                        "send_agent",
                        json!({"sessionId":session_id,"content":content}),
                    )
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn abort(&self) -> Result<()> {
        match self {
            Self::Local(agent) => agent.abort().await?,
            Self::Remote {
                session_id,
                transport,
            } => {
                transport
                    .request("abort_agent", json!({"sessionId":session_id}))
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn approve(&self, request_id: &str, approved: bool) -> Result<()> {
        match self {
            Self::Local(agent) => {
                let decision = if approved {
                    ApprovalDecision::Approve
                } else {
                    ApprovalDecision::Deny
                };
                agent.approve(request_id, decision)?;
            }
            Self::Remote {
                session_id,
                transport,
            } => {
                transport
                    .request(
                        "approve_agent",
                        json!({
                            "sessionId":session_id,"requestId":request_id,"approved":approved
                        }),
                    )
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn disconnect(&self) -> Result<()> {
        match self {
            Self::Local(agent) => agent.disconnect().await?,
            Self::Remote { transport, .. } => transport.shutdown().await,
        }
        Ok(())
    }
}

pub(crate) enum AgentEvents {
    Local(broadcast::Receiver<RuntimeEvent>),
    Remote(broadcast::Receiver<Value>),
}

impl AgentEvents {
    pub async fn recv(&mut self) -> Result<RuntimeEvent> {
        match self {
            Self::Local(incoming) => incoming
                .recv()
                .await
                .context("The runtime event stream was interrupted"),
            Self::Remote(incoming) => {
                let value = incoming
                    .recv()
                    .await
                    .context("The SSH session was disconnected")?;
                serde_json::from_value(value).context("The remote event format is invalid")
            }
        }
    }
}

/// Dropping a live session also cancels its application event forwarding task.
pub(crate) struct LiveSession {
    pub handle: AgentHandle,
    pub event_task: JoinHandle<()>,
    pub completion: TurnCompletion,
}

impl LiveSession {
    pub async fn stop_forwarding(&mut self) {
        abort_and_join(&mut self.event_task).await;
        self.completion.fail();
    }
}

async fn abort_and_join(task: &mut JoinHandle<()>) {
    task.abort();
    let _ = task.await;
}

/// A turn is complete only after its terminal event has been persisted and emitted.
#[derive(Clone)]
pub(crate) struct TurnCompletion(watch::Sender<CompletionState>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompletionState {
    Ready,
    Running,
    Failed,
}

impl TurnCompletion {
    pub fn new() -> Self {
        Self(watch::channel(CompletionState::Ready).0)
    }

    pub async fn begin(&self) -> Result<()> {
        self.wait().await?;
        let started = self.0.send_if_modified(|state| {
            if *state == CompletionState::Ready {
                *state = CompletionState::Running;
                true
            } else {
                false
            }
        });
        anyhow::ensure!(
            started,
            "The session connection is no longer valid. Try again."
        );
        Ok(())
    }

    /// Process an event and its completion state under the same synchronous lock.
    /// Returns whether the connection can continue forwarding events.
    /// The callback must not access this completion again.
    pub fn process_event(&self, receive: impl FnOnce() -> EventOutcome) -> bool {
        let mut keep_forwarding = false;
        self.0.send_if_modified(|state| {
            if *state == CompletionState::Failed {
                return false;
            }
            // Checking before the callback would let cancellation race with a late
            // idle writing to the store. fail() waits for this entire callback.
            let next = match receive() {
                EventOutcome::Continue => *state,
                EventOutcome::TurnFinished => CompletionState::Ready,
                EventOutcome::SessionFailed => CompletionState::Failed,
            };
            keep_forwarding = next != CompletionState::Failed;
            let changed = *state != next;
            *state = next;
            changed
        });
        keep_forwarding
    }

    pub fn fail(&self) {
        self.0.send_replace(CompletionState::Failed);
    }

    pub fn has_failed(&self) -> bool {
        *self.0.borrow() == CompletionState::Failed
    }

    pub async fn wait(&self) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut completed = self.0.subscribe();
            while *completed.borrow_and_update() == CompletionState::Running {
                completed
                    .changed()
                    .await
                    .context("Conversation event forwarding has stopped")?;
            }
            Ok(())
        })
        .await
        .context("The conversation completion event is still being processed. Try again shortly.")?
    }
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        self.event_task.abort();
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::oneshot;

    use super::*;

    #[tokio::test]
    async fn a_new_turn_waits_until_the_previous_terminal_event_is_handled() -> Result<()> {
        let completion = TurnCompletion::new();
        completion.begin().await?;
        let next_turn = completion.begin();
        tokio::pin!(next_turn);
        tokio::select! {
            result = &mut next_turn => anyhow::bail!("next turn bypassed the completion barrier: {result:?}"),
            () = tokio::task::yield_now() => {}
        }
        assert!(completion.process_event(|| EventOutcome::TurnFinished));
        next_turn.await?;
        assert_eq!(*completion.0.borrow(), CompletionState::Running);
        Ok(())
    }

    #[tokio::test]
    async fn failure_releases_waiters_without_allowing_the_connection_to_be_reused() -> Result<()> {
        let completion = TurnCompletion::new();
        completion.begin().await?;
        let next_turn = completion.begin();
        tokio::pin!(next_turn);
        tokio::select! {
            result = &mut next_turn => anyhow::bail!("next turn bypassed the completion barrier: {result:?}"),
            () = tokio::task::yield_now() => {}
        }
        completion.fail();
        assert!(next_turn.await.is_err());
        completion.wait().await?;
        assert!(!completion.process_event(|| panic!("failed connection processed an event")));
        assert!(completion.has_failed());
        assert!(completion.begin().await.is_err());
        Ok(())
    }

    #[test]
    fn invalidation_waits_for_in_progress_event_processing() -> Result<()> {
        let completion = TurnCompletion::new();
        std::thread::scope(|scope| -> Result<()> {
            let (entered, processing) = std::sync::mpsc::channel();
            let (release, resume) = std::sync::mpsc::channel();
            let event_completion = completion.clone();
            let event = scope.spawn(move || {
                event_completion.process_event(|| {
                    let _ = entered.send(());
                    if resume.recv_timeout(Duration::from_secs(5)).is_err() {
                        return EventOutcome::SessionFailed;
                    }
                    EventOutcome::TurnFinished
                })
            });
            processing.recv_timeout(Duration::from_secs(1))?;
            let (invalidating, started) = std::sync::mpsc::channel();
            let (invalidated, finished) = std::sync::mpsc::channel();
            let cancelled_completion = completion.clone();
            let cancellation = scope.spawn(move || {
                let _ = invalidating.send(());
                cancelled_completion.fail();
                let _ = invalidated.send(());
            });
            started.recv_timeout(Duration::from_secs(1))?;
            let returned_early = finished.recv_timeout(Duration::from_millis(20)).is_ok();
            release.send(())?;
            let processed = event
                .join()
                .map_err(|_| anyhow::anyhow!("event processing panicked"))?;
            cancellation
                .join()
                .map_err(|_| anyhow::anyhow!("cancellation panicked"))?;
            assert!(processed);
            assert!(!returned_early);
            assert!(completion.has_failed());
            Ok(())
        })
    }

    #[tokio::test]
    async fn stopping_forwarding_waits_for_the_task_to_release_its_resources() -> Result<()> {
        struct NotifyDrop(Option<oneshot::Sender<()>>);
        impl Drop for NotifyDrop {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }
        let (started, ready) = oneshot::channel();
        let (dropped, mut released) = oneshot::channel();
        let mut task = tokio::spawn(async move {
            let _guard = NotifyDrop(Some(dropped));
            let _ = started.send(());
            std::future::pending::<()>().await;
        });
        ready.await?;
        abort_and_join(&mut task).await;
        assert!(released.try_recv().is_ok());
        assert!(task.is_finished());
        Ok(())
    }
}
