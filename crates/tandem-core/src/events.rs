use std::sync::Arc;

use copilot_runtime::{ContextUsage, EventKind, RuntimeEvent};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    App, Error, Result,
    app::now,
    model::{Event, Message, MessageRole, SessionStatus},
};

pub(crate) enum EventOutcome {
    Continue,
    TurnFinished,
    SessionFailed,
}

impl App {
    /// Classifies the result only after persistence and application notification.
    pub(crate) fn receive_event(self: &Arc<Self>, event: &RuntimeEvent) -> EventOutcome {
        let result = self.persist_event(event);
        match result {
            Ok(Some((kind, data))) => {
                let outcome = if kind == "error" {
                    EventOutcome::SessionFailed
                } else if kind == "status" && data["status"] == "idle" {
                    EventOutcome::TurnFinished
                } else {
                    EventOutcome::Continue
                };
                if matches!(outcome, EventOutcome::TurnFinished)
                    && let Err(error) = self.schedule_title(&event.session_id, false)
                {
                    self.emit(
                        &event.session_id,
                        "title_error",
                        json!({"message":error.to_string()}),
                    );
                }
                self.emit(&event.session_id, kind, data);
                outcome
            }
            Ok(None) => EventOutcome::Continue,
            Err(error) => {
                self.fail_session(
                    &event.session_id,
                    &format!("Cannot save the conversation: {error}"),
                );
                EventOutcome::SessionFailed
            }
        }
    }

    fn persist_event(&self, event: &RuntimeEvent) -> Result<Option<(&'static str, Value)>> {
        let mut data = event.data.clone();
        let kind = match event.kind {
            EventKind::Message => {
                let content = data["content"].as_str().unwrap_or_default();
                if content.is_empty() {
                    return Ok(None);
                }
                let message = Message {
                    id: Uuid::new_v4().to_string(),
                    session_id: event.session_id.clone(),
                    role: MessageRole::Assistant,
                    content: content.into(),
                    created_at: now(),
                    interrupted: false,
                };
                self.store.append_message(&message)?;
                self.turns.completed(&event.session_id, &data)?;
                data = serde_json::to_value(message)?;
                "message"
            }
            EventKind::Status => {
                let status: SessionStatus = serde_json::from_value(data["state"].clone())
                    .map_err(|error| Error::InvalidRuntimeState(error.into()))?;
                if status == SessionStatus::Idle {
                    self.persist_interrupted(&event.session_id)?;
                }
                self.set_status(&event.session_id, status)?;
                data = json!({"status":status});
                "status"
            }
            EventKind::Error => {
                self.persist_interrupted(&event.session_id)?;
                self.set_status(&event.session_id, SessionStatus::Error)?;
                if data["message"].as_str().is_none() {
                    data = json!({"message":"The Copilot session encountered an error. Try again or check the provider settings."});
                }
                "error"
            }
            EventKind::Tool => {
                data = tool_activity(&data);
                "tool"
            }
            EventKind::Delta => {
                self.turns.delta(&event.session_id, &data)?;
                "delta"
            }
            EventKind::Permission => {
                self.turns.permission(&event.session_id, &data)?;
                "permission"
            }
            EventKind::ContextUsage => {
                let Some(usage) = ContextUsage::from_data(data) else {
                    return Ok(None);
                };
                if self.store.session(&event.session_id)?.project_id.is_none() {
                    return Ok(None);
                }
                self.store.update_session(&event.session_id, |session| {
                    session.context_usage = Some(usage);
                })?;
                data = json!(usage);
                "context_usage"
            }
        };
        Ok(Some((kind, data)))
    }

    pub(crate) fn set_status(&self, id: &str, status: SessionStatus) -> Result<()> {
        self.store.update_session(id, |session| {
            session.status = status;
            session.updated_at = now();
        })?;
        Ok(())
    }

    pub(crate) fn fail_session(&self, id: &str, message: &str) {
        if let Err(error) = self.persist_interrupted(id) {
            tracing::error!(%error, "could not save interrupted output");
        }
        if let Err(error) = self.set_status(id, SessionStatus::Error) {
            tracing::error!(%error, "could not update session status");
        }
        self.emit(id, "error", json!({"message":message}));
    }

    pub(crate) fn persist_interrupted(&self, id: &str) -> Result<()> {
        for message in self.turns.finish(id)? {
            self.store.append_message(&message)?;
            self.emit(id, "message", serde_json::to_value(message)?);
        }
        Ok(())
    }

    pub(crate) fn emit(&self, id: &str, kind: &str, data: Value) {
        let _ = self.events.send(Event {
            session_id: id.into(),
            kind: kind.into(),
            data,
        });
    }
}

fn tool_activity(data: &Value) -> Value {
    let details = &data["details"];
    let event_name = data["event"].as_str().unwrap_or_default();
    let name = details["toolName"].as_str().unwrap_or("tool");
    let error = details.get("error").filter(|error| !error.is_null());
    let failed = details["success"] == false || error.is_some();
    let content = error
        .or_else(|| details.get("result"))
        .or_else(|| details.get("arguments"))
        .map_or_else(String::new, Value::to_string);
    let status = if failed {
        "error"
    } else if event_name.contains("complete") {
        "completed"
    } else {
        "running"
    };
    json!({"name":name,"status":status,"content":content,"toolCallId":details["toolCallId"]})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_failures_retain_their_error_and_call_identity() {
        for (details, status, expected_content) in [
            (
                json!({"success":true,"result":"written"}),
                "completed",
                "written",
            ),
            (
                json!({"success":false,"error":{"message":"file exists"}}),
                "error",
                "file exists",
            ),
            (
                json!({"success":true,"error":"permission denied"}),
                "error",
                "permission denied",
            ),
            (
                json!({"success":false,"result":"tool rejected"}),
                "error",
                "tool rejected",
            ),
        ] {
            let mut details = details;
            details["toolCallId"] = json!("call-1");
            details["toolName"] = json!("create");
            let value =
                tool_activity(&json!({"event":"tool.execution_complete","details":details}));
            assert_eq!(value["status"], status);
            assert_eq!(value["toolCallId"], "call-1");
            assert!(
                value["content"]
                    .as_str()
                    .is_some_and(|content| content.contains(expected_content))
            );
        }
    }
}
