//! Stable events presented by the application, independent of SDK wire versions.

use github_copilot_sdk::SessionEvent;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Delta,
    Message,
    Reasoning,
    #[serde(rename = "reasoning_delta")]
    ReasoningDelta,
    Intent,
    Tool,
    Agent,
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
        if child_session_event(&event) {
            return None;
        }
        let (kind, mut data) = match event.event_type.as_str() {
            "assistant.message_delta" => (
                EventKind::Delta,
                json!({"content": event.data.get("deltaContent"), "messageId": event.data.get("messageId")}),
            ),
            "assistant.message" => (EventKind::Message, assistant_message_data(&event)),
            "assistant.intent" => (
                EventKind::Intent,
                json!({"intent": event.data.get("intent")}),
            ),
            "assistant.reasoning" | "assistant.reasoning_delta" => {
                let reasoning_id = event.data.get("reasoningId")?.as_str()?;
                let complete = event.event_type == "assistant.reasoning";
                let content = event
                    .data
                    .get(if complete { "content" } else { "deltaContent" })?
                    .as_str()?;
                if reasoning_id.is_empty() || (!complete && content.is_empty()) {
                    return None;
                }
                (
                    if complete {
                        EventKind::Reasoning
                    } else {
                        EventKind::ReasoningDelta
                    },
                    json!({"reasoningId": reasoning_id, "content": content}),
                )
            }
            "session.idle" => (
                EventKind::Status,
                json!({"state": "idle", "event": event.event_type}),
            ),
            "assistant.turn_start" => {
                let mut data = json!({"state": "running", "event": event.event_type});
                if let Some(turn_id) = event.data.get("turnId") {
                    data["turnId"] = turn_id.clone();
                }
                (EventKind::Status, data)
            }
            "session.usage_info" => {
                let mut usage = ContextUsage::from_data(event.data)?;
                // The SDK reports the prompt budget after reserving output tokens.
                // Display the configured total capacity while retaining its live count.
                usage.token_limit = context_window;
                (EventKind::ContextUsage, json!(usage))
            }
            "session.error" => (EventKind::Error, event.data),
            "subagent.started" | "subagent.completed" | "subagent.failed" => {
                let mut data = event.data;
                data.as_object_mut()?
                    .insert("event".into(), json!(event.event_type));
                (EventKind::Agent, data)
            }
            kind if kind.starts_with("tool.") => (
                EventKind::Tool,
                json!({"event": event.event_type, "details": event.data}),
            ),
            _ => return None,
        };
        if matches!(
            kind,
            EventKind::Delta
                | EventKind::Message
                | EventKind::Reasoning
                | EventKind::ReasoningDelta
                | EventKind::Intent
                | EventKind::Tool
                | EventKind::Agent
        ) {
            data["eventId"] = json!(event.id);
            data["timestamp"] = json!(event.timestamp);
            if let Some(agent_id) = event.agent_id {
                data["agentId"] = json!(agent_id);
            }
        }
        Some(Self {
            session_id: session_id.into(),
            kind,
            data,
        })
    }
}

fn child_session_event(event: &SessionEvent) -> bool {
    event.agent_id.as_deref().is_some_and(|id| !id.is_empty())
        && matches!(
            event.event_type.as_str(),
            "session.idle" | "assistant.turn_start" | "session.error" | "session.usage_info"
        )
}

fn assistant_message_data(event: &SessionEvent) -> Value {
    let mut data = json!({
        "content": event.data.get("content"),
        "id": event.data.get("messageId").unwrap_or(&json!(event.id)),
    });
    for field in [
        "phase",
        "toolRequests",
        "turnId",
        "apiCallId",
        "chunkIndex",
        "chunkCount",
        "originatingMessageId",
        "parentToolCallId",
    ] {
        if let Some(value) = event.data.get(field) {
            data[field] = value.clone();
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subagent_lifecycle_and_output_keep_distinct_worker_identities()
    -> Result<(), serde_json::Error> {
        for name in ["subagent.started", "subagent.completed", "subagent.failed"] {
            for worker in ["worker-1", "worker-2"] {
                let sdk = serde_json::from_value(json!({
                    "id":format!("event-{worker}"), "timestamp":"2026-10-21T00:00:00Z",
                    "type":name, "agentId":worker,
                    "data":{"toolCallId":"spawn-1", "agentName":"researcher", "parentId":"parent-1", "model":"fixture-model", "cancelled":true},
                }))?;
                let adapted = RuntimeEvent::from_sdk("session-1", sdk, 200_000);
                let event = adapted.as_ref();
                assert_eq!(event.map(|event| event.kind), Some(EventKind::Agent));
                assert_eq!(
                    event.map(|event| &event.data["agentId"]),
                    Some(&json!(worker))
                );
                assert_eq!(
                    event.map(|event| &event.data["toolCallId"]),
                    Some(&json!("spawn-1"))
                );
                assert_eq!(
                    event.map(|event| &event.data["parentId"]),
                    Some(&json!("parent-1"))
                );
                assert_eq!(event.map(|event| &event.data["event"]), Some(&json!(name)));
                assert_eq!(event.map(RuntimeEvent::ends_turn), Some(false));
            }
        }
        Ok(())
    }

    #[test]
    fn worker_session_events_cannot_finish_or_overwrite_the_parent_turn()
    -> Result<(), serde_json::Error> {
        for name in [
            "session.idle",
            "assistant.turn_start",
            "session.error",
            "session.usage_info",
        ] {
            let sdk = serde_json::from_value(json!({
                "id":"child-event", "timestamp":"2026-10-21T00:00:00Z",
                "type":name, "agentId":"worker-1",
                "data":{"message":"Worker error", "currentTokens":100,"tokenLimit":200_000},
            }))?;
            assert!(RuntimeEvent::from_sdk("session-1", sdk, 200_000).is_none());
        }
        Ok(())
    }

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
                    json!({"content": "hello", "messageId": "message-1", "eventId": "event-1", "timestamp": "2026-10-17T00:00:00Z"}),
                )),
            ),
            (
                "assistant.message",
                json!({"content": "hello"}),
                Some((
                    EventKind::Message,
                    false,
                    json!({"content": "hello", "id": "event-1", "eventId": "event-1", "timestamp": "2026-10-17T00:00:00Z"}),
                )),
            ),
            (
                "assistant.intent",
                json!({"intent": "Inspecting tool rendering"}),
                Some((
                    EventKind::Intent,
                    false,
                    json!({"intent": "Inspecting tool rendering", "eventId": "event-1", "timestamp": "2026-10-17T00:00:00Z"}),
                )),
            ),
            (
                "tool.execution_complete",
                json!({"success": false}),
                Some((
                    EventKind::Tool,
                    false,
                    json!({"event": "tool.execution_complete", "details": {"success": false}, "eventId": "event-1", "timestamp": "2026-10-17T00:00:00Z"}),
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
            ("assistant.turn_end", json!({"turnId": "turn-1"}), None),
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
    fn activity_events_keep_association_metadata_without_private_model_payloads()
    -> Result<(), serde_json::Error> {
        let timestamp = "2026-10-20T08:00:00Z";
        let tool_request = json!({
            "toolCallId": "tool-1",
            "name": "view",
            "arguments": {"path": "/project/src/main.rs"},
            "toolTitle": "Read source",
            "intentionSummary": "Inspect the application entry point",
        });
        for (event_type, payload, expected) in [
            (
                "assistant.message",
                json!({
                    "messageId": "message-1", "content": "I will inspect the entry point.",
                    "phase": "response", "turnId": "turn-1", "toolRequests": [tool_request],
                    "apiCallId": "api-1", "chunkIndex": 0, "chunkCount": 2,
                    "originatingMessageId": "user-1", "reasoningText": "private reasoning",
                    "encryptedContent": "private encrypted state", "providerId": "private provider",
                }),
                json!({
                    "id": "message-1", "content": "I will inspect the entry point.",
                    "phase": "response", "turnId": "turn-1", "toolRequests": [tool_request],
                    "apiCallId": "api-1", "chunkIndex": 0, "chunkCount": 2,
                    "originatingMessageId": "user-1", "eventId": "sdk-1",
                    "timestamp": timestamp, "agentId": "agent-1",
                }),
            ),
            (
                "assistant.message_delta",
                json!({"messageId": "message-1", "deltaContent": "Inspecting", "reasoningText": "private reasoning"}),
                json!({"messageId": "message-1", "content": "Inspecting", "eventId": "sdk-1", "timestamp": timestamp, "agentId": "agent-1"}),
            ),
            (
                "assistant.intent",
                json!({"intent": "Inspecting the application", "reasoningText": "private reasoning"}),
                json!({"intent": "Inspecting the application", "eventId": "sdk-1", "timestamp": timestamp, "agentId": "agent-1"}),
            ),
        ] {
            let sdk = serde_json::from_value(json!({
                "id": "sdk-1", "timestamp": timestamp, "agentId": "agent-1",
                "type": event_type, "data": payload,
            }))?;
            let adapted = RuntimeEvent::from_sdk("session-1", sdk, 200_000);
            assert_eq!(
                adapted.map(|event| event.data),
                Some(expected),
                "{event_type}"
            );
        }
        Ok(())
    }

    #[test]
    fn reasoning_events_only_forward_explicit_display_text() -> Result<(), serde_json::Error> {
        for (event_type, content_field, kind) in [
            ("assistant.reasoning", "content", EventKind::Reasoning),
            (
                "assistant.reasoning_delta",
                "deltaContent",
                EventKind::ReasoningDelta,
            ),
        ] {
            let mut data = json!({
                "reasoningId": "reasoning-1",
                "reasoningOpaque": "opaque state",
                "encryptedContent": "encrypted state",
                "reasoningBlocks": {"provider": "opaque state"},
            });
            data[content_field] = json!("Inspecting the files");
            let sdk = serde_json::from_value(json!({
                "id": "event-1", "timestamp": "2026-10-21T00:00:00Z", "agentId": "agent-1",
                "type": event_type, "data": data,
            }))?;
            let adapted = RuntimeEvent::from_sdk("session-1", sdk, 200_000);
            assert_eq!(
                adapted.map(|event| (event.kind, event.data)),
                Some((
                    kind,
                    json!({
                        "reasoningId": "reasoning-1", "content": "Inspecting the files",
                        "eventId": "event-1", "timestamp": "2026-10-21T00:00:00Z", "agentId": "agent-1",
                    })
                )),
            );
        }
        for data in [
            json!({"reasoningId": "", "deltaContent": "Text"}),
            json!({"reasoningId": "reasoning-1", "deltaContent": ""}),
            json!({"reasoningId": "reasoning-1", "deltaContent": {"text": "Text"}}),
            json!({"reasoningId": 1, "deltaContent": "Text"}),
            json!({"reasoningOpaque": "opaque state"}),
        ] {
            let sdk = serde_json::from_value(json!({
                "id": "event-1", "timestamp": "2026-10-21T00:00:00Z",
                "type": "assistant.reasoning_delta", "data": data,
            }))?;
            assert!(RuntimeEvent::from_sdk("session-1", sdk, 200_000).is_none());
        }
        Ok(())
    }

    #[test]
    fn tool_lifecycle_keeps_call_identity_and_iteration_end_does_not_finish_work()
    -> Result<(), serde_json::Error> {
        for (event_type, details) in [
            (
                "tool.execution_start",
                json!({"toolCallId": "tool-1", "toolName": "view", "toolTitle": "Read source", "turnId": "turn-1", "arguments": {"path": "/project/src/main.rs"}}),
            ),
            (
                "tool.execution_progress",
                json!({"toolCallId": "tool-1", "progressMessage": "Reading source"}),
            ),
            (
                "tool.execution_complete",
                json!({"toolCallId": "tool-1", "turnId": "turn-1", "success": false, "error": {"message": "File is unavailable", "code": "not_found"}}),
            ),
        ] {
            let sdk = serde_json::from_value(json!({
                "id": "sdk-1", "timestamp": "2026-10-20T08:00:00Z", "agentId": "agent-1",
                "type": event_type, "data": details,
            }))?;
            let adapted = RuntimeEvent::from_sdk("session-1", sdk, 200_000);
            assert!(adapted.as_ref().is_some_and(|event| !event.ends_turn()));
            assert_eq!(
                adapted.map(|event| (event.kind, event.data)),
                Some((
                    EventKind::Tool,
                    json!({
                        "event": event_type, "details": details,
                        "eventId": "sdk-1", "timestamp": "2026-10-20T08:00:00Z", "agentId": "agent-1",
                    })
                )),
            );
        }
        let sdk = serde_json::from_value(json!({
            "id": "sdk-2", "timestamp": "2026-10-20T08:00:00Z",
            "type": "assistant.turn_start", "data": {"turnId": "turn-2"},
        }))?;
        let adapted = RuntimeEvent::from_sdk("session-1", sdk, 200_000);
        assert!(adapted.as_ref().is_some_and(|event| !event.ends_turn()));
        assert_eq!(
            adapted.map(|event| event.data),
            Some(json!({
                "state": "running", "event": "assistant.turn_start", "turnId": "turn-2",
            }))
        );
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
