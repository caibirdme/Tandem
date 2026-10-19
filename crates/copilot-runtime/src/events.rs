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
    #[serde(rename = "context_usage")]
    ContextUsage,
    Error,
}

/// The latest context-window reading, not accumulated inference billing tokens.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    pub current_tokens: i64,
    pub token_limit: i64,
}

impl ContextUsage {
    /// Decode only usable, exactly representable token counts from a runtime event.
    #[must_use]
    pub fn from_data(data: Value) -> Option<Self> {
        const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
        let usage: Self = serde_json::from_value(data).ok()?;
        ((0..=MAX_SAFE_INTEGER).contains(&usage.current_tokens)
            && (1..=MAX_SAFE_INTEGER).contains(&usage.token_limit))
        .then_some(usage)
    }
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

    pub(crate) fn from_sdk(
        session_id: &str,
        event: SessionEvent,
        context_window: i64,
    ) -> Option<Self> {
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
            "session.usage_info" => {
                let mut usage = ContextUsage::from_data(event.data)?;
                // The SDK reports the prompt budget after reserving output tokens.
                // Display the configured total capacity while retaining its live count.
                usage.token_limit = context_window;
                (EventKind::ContextUsage, json!(usage))
            }
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
            (
                "session.usage_info",
                json!({"currentTokens":153_000,"tokenLimit":1_000_000,"messagesLength":4}),
                Some((
                    EventKind::ContextUsage,
                    false,
                    json!({"currentTokens":153_000,"tokenLimit":1_000_000}),
                )),
            ),
            (
                "assistant.usage",
                json!({"inputTokens":153_000,"outputTokens":2_000}),
                None,
            ),
            ("future.sdk_event", json!({}), None),
        ] {
            let sdk = serde_json::from_value(json!({
                "id": "event-1", "timestamp": "2026-10-17T00:00:00Z",
                "type": event_type, "data": data,
            }))?;
            let adapted = RuntimeEvent::from_sdk("session-1", sdk, 1_000_000);
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

    #[test]
    fn context_usage_uses_total_capacity_instead_of_the_sdk_prompt_budget()
    -> Result<(), serde_json::Error> {
        for (context_window, prompt_budget) in [(256_000, 251_904), (1_000_000, 995_904)] {
            let event = serde_json::from_value(json!({
                "id":"usage-1","timestamp":"2026-10-19T00:00:00Z",
                "type":"session.usage_info",
                "data":{"currentTokens":153_000,"tokenLimit":prompt_budget,"messagesLength":4},
            }))?;
            let adapted = RuntimeEvent::from_sdk("session-1", event, context_window);
            assert_eq!(
                adapted.map(|event| event.data),
                Some(json!({
                    "currentTokens":153_000,"tokenLimit":context_window,
                }))
            );
        }
        Ok(())
    }

    #[test]
    fn context_usage_rejects_invalid_counts_and_retains_over_limit_readings() {
        for (data, valid) in [
            (json!({"currentTokens":0,"tokenLimit":1_000_000}), true),
            (
                json!({"currentTokens":1_000_001,"tokenLimit":1_000_000}),
                true,
            ),
            (json!({"currentTokens":-1,"tokenLimit":1_000_000}), false),
            (json!({"currentTokens":153_000,"tokenLimit":0}), false),
            (json!({"currentTokens":153_000,"tokenLimit":-1}), false),
            (json!({"currentTokens":1.5,"tokenLimit":1_000_000}), false),
            (
                json!({"currentTokens":9_007_199_254_740_992_i64,"tokenLimit":1_000_000}),
                false,
            ),
            (
                json!({"currentTokens":"153000","tokenLimit":1_000_000}),
                false,
            ),
            (json!({"currentTokens":153_000}), false),
            (json!({"tokenLimit":1_000_000}), false),
        ] {
            assert_eq!(
                ContextUsage::from_data(data.clone()).is_some(),
                valid,
                "{data}"
            );
        }
    }
}
