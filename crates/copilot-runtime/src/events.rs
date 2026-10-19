//! Stable events presented by the application, independent of SDK wire versions.

use github_copilot_sdk::SessionEvent;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Delta,
    Message,
    Tool,
    Permission,
    Status,
    Error,
}

/// Events contain no provider configuration or authentication headers.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEvent {
    pub session_id: String,
    pub kind: EventKind,
    pub data: Value,
}

impl RuntimeEvent {
    pub(crate) fn ends_turn(&self) -> bool {
        self.kind == EventKind::Error
            || (self.kind == EventKind::Status && self.data["state"] == "idle")
    }

    pub(crate) fn from_sdk(session_id: &str, event: SessionEvent) -> Option<Self> {
        let (kind, data) = match event.event_type.as_str() {
            "assistant.message_delta" => (
                EventKind::Delta,
                json!({"content": event.data.get("deltaContent"), "messageId": event.data.get("messageId")}),
            ),
            "assistant.message" => (
                EventKind::Message,
                json!({"content": event.data.get("content"), "id": event.data.get("messageId").unwrap_or(&json!(event.id))}),
            ),
            "session.idle" => (
                EventKind::Status,
                json!({"state": "idle", "event": event.event_type}),
            ),
            "assistant.turn_start" => (
                EventKind::Status,
                json!({"state": "running", "event": event.event_type}),
            ),
            "session.error" => (EventKind::Error, event.data),
            kind if kind.starts_with("tool.") => (
                EventKind::Tool,
                json!({"event": event.event_type, "details": event.data}),
            ),
            _ => return None,
        };
        Some(Self {
            session_id: session_id.into(),
            kind,
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_events_preserve_identity_and_only_terminal_events_finish_turns()
    -> Result<(), serde_json::Error> {
        for (event_type, data, expected) in [
            (
                "assistant.message_delta",
                json!({"deltaContent": "hello", "messageId": "message-1"}),
                Some((
                    EventKind::Delta,
                    false,
                    json!({"content": "hello", "messageId": "message-1"}),
                )),
            ),
            (
                "assistant.message",
                json!({"content": "hello"}),
                Some((
                    EventKind::Message,
                    false,
                    json!({"content": "hello", "id": "event-1"}),
                )),
            ),
            (
                "tool.execution_complete",
                json!({"success": false}),
                Some((
                    EventKind::Tool,
                    false,
                    json!({"event": "tool.execution_complete", "details": {"success": false}}),
                )),
            ),
            (
                "session.idle",
                json!({}),
                Some((
                    EventKind::Status,
                    true,
                    json!({"state": "idle", "event": "session.idle"}),
                )),
            ),
            (
                "session.error",
                json!({"message": "disconnected"}),
                Some((EventKind::Error, true, json!({"message": "disconnected"}))),
            ),
            ("future.sdk_event", json!({}), None),
        ] {
            let sdk = serde_json::from_value(json!({
                "id": "event-1", "timestamp": "2026-10-17T00:00:00Z",
                "type": event_type, "data": data,
            }))?;
            let adapted = RuntimeEvent::from_sdk("session-1", sdk);
            assert_eq!(
                adapted.as_ref().map(|event| event.session_id.as_str()),
                expected.as_ref().map(|_| "session-1")
            );
            assert_eq!(
                adapted.map(|event| (event.kind, event.ends_turn(), event.data)),
                expected,
                "{event_type}"
            );
        }
        Ok(())
    }
}
