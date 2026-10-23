//! Preserve the busy boundary until accepted guidance reaches the runtime.

use std::collections::HashSet;

use github_copilot_sdk::SessionEvent;

#[derive(Default)]
pub(crate) struct Steering {
    generation: u64,
    admitting: bool,
    observed: HashSet<String>,
    pending: HashSet<String>,
    deferred_idle: Option<SessionEvent>,
}

impl Steering {
    pub(crate) fn begin(&mut self) -> u64 {
        self.admitting = true;
        self.observed.clear();
        self.generation
    }

    /// Returns false only for an idle event that precedes consumption of guidance.
    pub(crate) fn observe(&mut self, event: &SessionEvent) -> bool {
        if event.agent_id.as_ref().is_some_and(|id| !id.is_empty()) {
            return true;
        }
        match event.event_type.as_str() {
            "session.error" => self.clear(),
            "session.idle" if event.data["aborted"] == true => self.clear(),
            "session.idle" if self.admitting || !self.pending.is_empty() => {
                self.deferred_idle = Some(event.clone());
                return false;
            }
            "user.message" => self.consumed(event.data["messageId"].as_str()),
            "session.warning" if event.data["warningType"] == "policy_blocked" => {
                self.consumed(event.data["originatingMessageId"].as_str());
            }
            _ => {}
        }
        true
    }

    fn consumed(&mut self, id: Option<&str>) {
        let Some(id) = id.filter(|id| !id.is_empty()) else {
            return;
        };
        if self.admitting {
            self.observed.insert(id.to_owned());
        }
        if self.pending.remove(id) || self.admitting {
            // This event starts work after a deferred idle. Wait for its own idle,
            // rather than releasing the previous turn's terminal event.
            self.deferred_idle = None;
        }
    }

    pub(crate) fn acknowledge(
        &mut self,
        generation: u64,
        id: Option<&str>,
    ) -> Option<SessionEvent> {
        if generation != self.generation {
            return None;
        }
        self.admitting = false;
        if let Some(id) = id.filter(|id| !id.is_empty())
            && !self.observed.contains(id)
        {
            self.pending.insert(id.to_owned());
        }
        self.observed.clear();
        if self.pending.is_empty() {
            self.deferred_idle.take()
        } else {
            None
        }
    }

    pub(crate) fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.admitting = false;
        self.observed.clear();
        self.pending.clear();
        self.deferred_idle = None;
    }
}

/// An admission reservation survives asynchronous validation and is released
/// if the caller abandons its prepared message before sending it.
pub(crate) struct Admission {
    state: std::sync::Arc<std::sync::Mutex<Steering>>,
    lifecycle: std::sync::Arc<crate::lifecycle::SessionLifecycle>,
    events: tokio::sync::broadcast::Sender<crate::RuntimeEvent>,
    session_id: String,
    generation: u64,
    starts_new_turn: bool,
    settled: bool,
    dispatched: bool,
}

impl Admission {
    pub(crate) fn new(
        state: std::sync::Arc<std::sync::Mutex<Steering>>,
        lifecycle: std::sync::Arc<crate::lifecycle::SessionLifecycle>,
        events: tokio::sync::broadcast::Sender<crate::RuntimeEvent>,
        session_id: String,
    ) -> crate::Result<Self> {
        let mut tracker = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !lifecycle.is_connected() {
            return Err(crate::Error::Disconnected);
        }
        if tracker.admitting {
            // An unresolved acknowledgement must survive later attempts. Only
            // abort/error/disconnection can safely clear that uncertainty.
            return Err(crate::Error::Busy);
        }
        let starts_new_turn = !lifecycle.is_running();
        let generation = tracker.begin();
        drop(tracker);
        Ok(Self {
            state,
            lifecycle,
            events,
            session_id,
            generation,
            starts_new_turn,
            settled: false,
            dispatched: false,
        })
    }

    pub(crate) fn starts_new_turn(&self) -> bool {
        self.starts_new_turn
    }

    pub(crate) fn dispatch(&mut self) -> crate::Result<bool> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.generation != self.generation {
            return Err(crate::ConfigurationError::PreparedSteeringCanceled.into());
        }
        let started = self.lifecycle.begin_steering()?;
        self.dispatched = true;
        Ok(started)
    }

    pub(crate) fn settle(mut self, id: Option<&str>) {
        self.release(id);
        self.settled = true;
    }

    fn release(&self, id: Option<&str>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.generation != self.generation {
            return;
        }
        if let Some(idle) = state.acknowledge(self.generation, id) {
            self.lifecycle.finish_turn();
            if let Some(event) = crate::RuntimeEvent::from_sdk(&self.session_id, idle, 0) {
                let _ = self.events.send(event);
            }
        } else if id.is_none() && self.starts_new_turn && state.pending.is_empty() {
            // Core may have reserved its new turn before persistence failed.
            // Preparation was abandoned or the SDK explicitly rejected it;
            // releasing this idle admission cannot cancel accepted work.
            self.lifecycle.finish_turn();
            let _ = self.events.send(crate::RuntimeEvent {
                session_id: self.session_id.clone(),
                kind: crate::EventKind::Status,
                data: serde_json::json!({"state":"idle", "event":"steering.discarded"}),
            });
        }
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        if !self.settled && !self.dispatched {
            self.release(None);
        }
        // A canceled send future may already be on the wire. Keep admission
        // reserved until an explicit abort/error rather than permit a blind retry.
    }
}
#[cfg(test)]
mod tests {
    use serde_json::json;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    use super::*;

    macro_rules! sdk_event {
        ($kind:expr, $data:expr) => {
            serde_json::from_value::<SessionEvent>(json!({
                "id":"event", "timestamp":"2026-10-23T00:00:00Z", "type":$kind, "data":$data,
            }))?
        };
    }

    #[test]
    fn abandoned_preparation_releases_the_deferred_idle() -> TestResult {
        let lifecycle = std::sync::Arc::new(crate::lifecycle::SessionLifecycle::new());
        lifecycle.begin_turn()?;
        let state = std::sync::Arc::new(std::sync::Mutex::new(Steering::default()));
        let (events, mut incoming) = tokio::sync::broadcast::channel(4);
        let admission = Admission::new(state.clone(), lifecycle.clone(), events, "session".into())?;
        assert!(!admission.starts_new_turn());
        assert!(
            !state
                .lock()
                .map_err(|_| "poisoned test mutex")?
                .observe(&sdk_event!("session.idle", json!({})))
        );
        assert!(lifecycle.is_running());
        assert!(incoming.try_recv().is_err());
        drop(admission);
        assert_eq!(incoming.try_recv()?.data["state"], "idle");
        assert!(!lifecycle.is_running());
        Ok(())
    }

    #[test]
    fn preparation_records_an_already_idle_runtime_without_starting_inference() -> TestResult {
        let lifecycle = std::sync::Arc::new(crate::lifecycle::SessionLifecycle::new());
        let state = std::sync::Arc::new(std::sync::Mutex::new(Steering::default()));
        let (events, mut incoming) = tokio::sync::broadcast::channel(4);
        let admission = Admission::new(state, lifecycle.clone(), events, "session".into())?;
        assert!(admission.starts_new_turn());
        assert!(!lifecycle.is_running());
        drop(admission);
        assert_eq!(incoming.try_recv()?.data["state"], "idle");
        assert!(!lifecycle.is_running());
        Ok(())
    }

    #[test]
    fn idle_before_consumption_is_suppressed_across_acknowledgement() -> TestResult {
        let mut steering = Steering::default();
        let token = steering.begin();
        assert!(!steering.observe(&sdk_event!("session.idle", json!({}))));
        assert!(steering.acknowledge(token, Some("guidance")).is_none());
        assert!(!steering.observe(&sdk_event!("session.idle", json!({}))));
        steering.observe(&sdk_event!("user.message", json!({"messageId":"guidance"})));
        assert!(steering.observe(&sdk_event!("session.idle", json!({}))));
        Ok(())
    }

    #[test]
    fn events_before_ack_release_only_the_guidance_terminal_idle() -> TestResult {
        let mut steering = Steering::default();
        let token = steering.begin();
        assert!(!steering.observe(&sdk_event!("session.idle", json!({}))));
        steering.observe(&sdk_event!("user.message", json!({"messageId":"guidance"})));
        assert!(steering.acknowledge(token, Some("guidance")).is_none());
        let token = steering.begin();
        steering.observe(&sdk_event!("user.message", json!({"messageId":"second"})));
        assert!(!steering.observe(&sdk_event!("session.idle", json!({}))));
        assert!(steering.acknowledge(token, Some("second")).is_some());
        Ok(())
    }

    #[test]
    fn policy_rejection_and_abort_release_admission() -> TestResult {
        let mut steering = Steering::default();
        let token = steering.begin();
        steering.acknowledge(token, Some("blocked"));
        assert!(!steering.observe(&sdk_event!("session.idle", json!({}))));
        steering.observe(&sdk_event!(
            "session.warning",
            json!({
                "warningType":"policy_blocked", "originatingMessageId":"blocked",
            })
        ));
        assert!(steering.observe(&sdk_event!("session.idle", json!({}))));
        let lifecycle = std::sync::Arc::new(crate::lifecycle::SessionLifecycle::new());
        let state = std::sync::Arc::new(std::sync::Mutex::new(Steering::default()));
        let (events, _) = tokio::sync::broadcast::channel(4);
        let mut admission = Admission::new(state.clone(), lifecycle, events, "session".into())?;
        state
            .lock()
            .map_err(|_| "poisoned test mutex")?
            .observe(&sdk_event!("session.idle", json!({"aborted":true})));
        assert!(matches!(
            admission.dispatch(),
            Err(crate::Error::InvalidConfiguration(
                crate::ConfigurationError::PreparedSteeringCanceled
            ))
        ));
        Ok(())
    }

    #[test]
    fn canceling_a_dispatched_future_does_not_advertise_false_idle() -> TestResult {
        let lifecycle = std::sync::Arc::new(crate::lifecycle::SessionLifecycle::new());
        let state = std::sync::Arc::new(std::sync::Mutex::new(Steering::default()));
        let (events, mut incoming) = tokio::sync::broadcast::channel(4);
        let mut admission =
            Admission::new(state.clone(), lifecycle.clone(), events, "session".into())?;
        admission.dispatch()?;
        drop(admission);
        assert!(lifecycle.is_running());
        assert!(incoming.try_recv().is_err());
        let mut state = state.lock().map_err(|_| "poisoned test mutex")?;
        assert!(!state.observe(&sdk_event!("session.idle", json!({}))));
        assert!(state.observe(&sdk_event!("session.idle", json!({"aborted":true}))));
        Ok(())
    }

    #[test]
    fn multiple_messages_and_abort_do_not_reopen_a_completed_session() -> TestResult {
        let mut steering = Steering::default();
        let first = steering.begin();
        steering.acknowledge(first, Some("first"));
        let second = steering.begin();
        steering.acknowledge(second, Some("second"));
        steering.observe(&sdk_event!("user.message", json!({"messageId":"first"})));
        assert!(!steering.observe(&sdk_event!("session.idle", json!({}))));
        let late = steering.begin();
        assert!(steering.observe(&sdk_event!("session.idle", json!({"aborted":true}))));
        assert!(steering.acknowledge(late, Some("late")).is_none());
        assert!(steering.observe(&sdk_event!("session.idle", json!({}))));
        Ok(())
    }
}
