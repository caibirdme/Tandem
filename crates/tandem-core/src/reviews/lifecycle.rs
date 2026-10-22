use std::time::{Duration, SystemTime, UNIX_EPOCH};

use copilot_runtime::workspace::ReviewSelection;
use serde_json::json;
use tokio::sync::{broadcast, watch};

use super::{ActiveReview, SessionRecord, TurnRecord, describe_unavailable};
use crate::{
    App, Error, Result,
    agent::AgentHandle,
    model::{Event, MessageRole, Session},
    store::Store,
};

const CAPTURE_BUDGET: Duration = Duration::from_secs(6);

impl App {
    pub(crate) async fn interrupt_review_turn(&self, session_id: &str) {
        if let Some(active) = self.reviews.state.lock().await.active.get_mut(session_id) {
            active.record.complete = false;
        }
    }

    pub(crate) async fn wait_review_capture(&self, session_id: &str) {
        let pending = self
            .reviews
            .state
            .lock()
            .await
            .finishing
            .get(session_id)
            .cloned();
        if let Some(pending) = pending {
            wait(pending).await;
        }
    }

    pub(crate) async fn begin_review_turn(
        &self,
        session: &Session,
        message_id: &str,
        agent: &AgentHandle,
    ) {
        if session.project_id.is_none() {
            return;
        }
        self.wait_review_capture(&session.id).await;
        let mut record = TurnRecord {
            message_id: message_id.into(),
            captured_at: now_millis(),
            before: None,
            after: None,
            complete: false,
            unavailable: None,
            summary: None,
        };
        let capture = async {
            let handle = self.review_location(session, Some(agent)).await?;
            let previous = self
                .store
                .review_turns::<TurnRecord>(&session.id)?
                .into_iter()
                .rev()
                .find_map(|record| record.after.or(record.before));
            let snapshot = handle.capture(previous.as_deref()).await?;
            Ok::<_, Error>((handle, snapshot))
        };
        let handle = match tokio::time::timeout(CAPTURE_BUDGET, capture)
            .await
            .map_err(|_| Error::Review(copilot_runtime::workspace::ReviewError::Timeout))
            .and_then(std::convert::identity)
        {
            Ok((handle, snapshot)) => {
                record.captured_at = snapshot.captured_at;
                record.complete = snapshot.complete;
                record.before = Some(snapshot.id);
                Some(handle)
            }
            Err(error) => {
                record.unavailable = Some(describe_unavailable(&error));
                None
            }
        };
        self.reviews.state.lock().await.active.insert(
            session.id.clone(),
            ActiveReview {
                handle,
                record,
                committed: false,
            },
        );
    }

    pub(crate) async fn commit_review_turn(&self, session_id: &str, message_id: &str) {
        let mut state = self.reviews.state.lock().await;
        let Some(active) = state
            .active
            .get_mut(session_id)
            .filter(|active| active.record.message_id == message_id)
        else {
            return;
        };
        active.committed = true;
        if self
            .commit_review_record(session_id, &active.record)
            .is_err()
        {
            active.record.unavailable = Some(super::ReviewUnavailable {
                code: super::UnavailableCode::InspectionFailed,
                message: "The review baseline could not be saved.".into(),
            });
        }
        drop(state);
        self.emit(session_id, "review_updated", json!({}));
    }

    fn commit_review_record(&self, session_id: &str, record: &TurnRecord) -> Result<()> {
        let mut session =
            if let Some(session) = self.store.review_session::<SessionRecord>(session_id)? {
                session
            } else {
                let previous = self.store.messages(session_id)?.iter().any(|message| {
                    message.role == MessageRole::User && message.id != record.message_id
                });
                SessionRecord {
                    baseline: if previous {
                        None
                    } else {
                        record.before.clone()
                    },
                    latest: None,
                }
            };
        session.latest = None;
        self.store.save_review_session(session_id, &session)?;
        let mut pending = record.clone();
        pending.complete = false;
        self.store
            .save_review_turn(session_id, &record.message_id, &pending)?;
        Ok(())
    }

    pub(crate) async fn abandon_review_turn(&self, session_id: &str, message_id: &str) {
        let mut state = self.reviews.state.lock().await;
        if state
            .active
            .get(session_id)
            .is_some_and(|active| !active.committed && active.record.message_id == message_id)
        {
            state.active.remove(session_id);
        }
    }

    pub(crate) async fn finish_review_turn(&self, session_id: &str, completed: bool) {
        let pending = {
            let mut state = self.reviews.state.lock().await;
            if let Some(pending) = state.finishing.get(session_id) {
                Some(pending.clone())
            } else if state
                .active
                .get(session_id)
                .is_some_and(|active| active.committed)
            {
                let Some(active) = state.active.remove(session_id) else {
                    return;
                };
                let (finished, pending) = watch::channel(false);
                state.finishing.insert(session_id.into(), pending.clone());
                let state = self.reviews.state.clone();
                let store = self.store.clone();
                let events = self.events.clone();
                let session_id = session_id.to_owned();
                tokio::spawn(async move {
                    finish_record(&store, &events, &session_id, active, completed).await;
                    state.lock().await.finishing.remove(&session_id);
                    finished.send_replace(true);
                });
                Some(pending)
            } else {
                None
            }
        };
        if let Some(pending) = pending {
            wait(pending).await;
        }
    }
}

async fn finish_record(
    store: &Store,
    events: &broadcast::Sender<Event>,
    session_id: &str,
    mut active: ActiveReview,
    completed: bool,
) {
    let result = tokio::time::timeout(
        CAPTURE_BUDGET,
        capture_end(store, session_id, &mut active, completed),
    )
    .await
    .map_err(|_| Error::Review(copilot_runtime::workspace::ReviewError::Timeout))
    .and_then(std::convert::identity);
    if let Err(error) = result {
        active.record.complete = false;
        if let Some(summary) = active.record.summary.as_mut() {
            summary.complete = false;
        }
        active.record.unavailable = Some(describe_unavailable(&error));
    }
    let _ = store.save_review_turn(session_id, &active.record.message_id, &active.record);
    let _ = events.send(Event {
        session_id: session_id.into(),
        kind: "review_updated".into(),
        data: json!({"messageId":active.record.message_id}),
    });
}

async fn capture_end(
    store: &Store,
    session_id: &str,
    active: &mut ActiveReview,
    completed: bool,
) -> Result<()> {
    let Some(handle) = &active.handle else {
        return Ok(());
    };
    let snapshot = handle.capture(active.record.before.as_deref()).await?;
    active.record.after = Some(snapshot.id.clone());
    active.record.complete = active.record.complete && snapshot.complete && completed;
    let Some(before) = &active.record.before else {
        return Ok(());
    };
    let mut summary = handle
        .compare(
            before,
            &snapshot.id,
            ReviewSelection::Turn {
                message_id: active.record.message_id.clone(),
            },
        )
        .await?;
    summary.complete &= active.record.complete;
    store.save_review_result(session_id, &summary.review_id, &summary)?;
    active.record.summary = Some(summary);
    let Some(mut session) = store.review_session::<SessionRecord>(session_id)? else {
        return Ok(());
    };
    if let Some(baseline) = &session.baseline {
        let mut summary = handle
            .compare(baseline, &snapshot.id, ReviewSelection::Session)
            .await?;
        summary.complete &= active.record.complete;
        store.save_review_result(session_id, &summary.review_id, &summary)?;
        session.latest = Some(summary);
        store.save_review_session(session_id, &session)?;
    }
    Ok(())
}

async fn wait(mut pending: watch::Receiver<bool>) {
    while !*pending.borrow_and_update() {
        if pending.changed().await.is_err() {
            break;
        }
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}
