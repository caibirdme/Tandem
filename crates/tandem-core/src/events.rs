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
        self.finish_reasoning_phase(event)?;
        let mut data = event.data.clone();
        let kind = match event.kind {
            EventKind::Reasoning | EventKind::ReasoningDelta => {
                self.publish_reasoning(event)?;
                return Ok(None);
            }
            EventKind::Message => {
                let Some(message) = self.persist_assistant_message(event)? else {
                    return Ok(None);
                };
                data = serde_json::to_value(message)?;
                "message"
            }
            EventKind::Agent => {
                let Some(message) = self.turns.agent(&event.session_id, &data)? else {
                    return Ok(None);
                };
                let message = self.store.save_agent(&message)?;
                self.turns.agent_saved(&message)?;
                if matches!(
                    data["event"].as_str(),
                    Some("subagent.completed" | "subagent.failed")
                ) {
                    self.save_finished_messages(
                        self.turns.finish_agent(&event.session_id, &data)?,
                    )?;
                }
                data = serde_json::to_value(message)?;
                "message"
            }
            EventKind::Intent => {
                self.turns.intent(&event.session_id, &data)?;
                return Ok(None);
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
                let Some(message) = self.turns.tool(&event.session_id, &data)? else {
                    return Ok(None);
                };
                let message = self.store.save_activity(&message)?;
                data = serde_json::to_value(message)?;
                "message"
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

    fn persist_assistant_message(&self, event: &RuntimeEvent) -> Result<Option<Message>> {
        let data = &event.data;
        let content = data["content"].as_str().unwrap_or_default();
        if content.is_empty() {
            self.turns.completed(&event.session_id, data)?;
            return Ok(None);
        }
        let checkpoint = self.turns.checkpointed_message(&event.session_id, data)?;
        let message = Message {
            id: checkpoint
                .as_ref()
                .map_or_else(|| Uuid::new_v4().to_string(), |message| message.id.clone()),
            session_id: event.session_id.clone(),
            role: MessageRole::Assistant,
            content: content.into(),
            attachments: Vec::new(),
            created_at: checkpoint
                .as_ref()
                .map_or_else(now, |message| message.created_at.clone()),
            sequence: None,
            activity: None,
            reasoning: None,
            selected_skills: Vec::new(),
            agent_id: data["agentId"].as_str().map(str::to_owned),
            agent: None,
            fleet: false,
            interrupted: false,
        };
        let message = if checkpoint.is_some() {
            self.store.save_assistant(&message)?
        } else {
            self.store.append_message(&message)?
        };
        self.turns.completed(&event.session_id, data)?;
        self.turns.assistant(message.clone(), data)?;
        Ok(Some(message))
    }

    fn finish_reasoning_phase(&self, event: &RuntimeEvent) -> Result<()> {
        let data = &event.data;
        let completed = match event.kind {
            EventKind::Delta | EventKind::Message
                if data["content"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()) =>
            {
                self.turns.close_reasoning(&event.session_id, data)?
            }
            EventKind::Intent
                if data["intent"]
                    .as_str()
                    .is_some_and(|text| !text.trim().is_empty()) =>
            {
                self.turns.close_reasoning(&event.session_id, data)?
            }
            EventKind::Tool if data["event"] == "tool.execution_start" => {
                self.turns.close_reasoning(&event.session_id, data)?
            }
            EventKind::Permission => self.turns.pause_reasoning(&event.session_id)?,
            _ => Vec::new(),
        };
        for message in completed {
            self.save_reasoning(&message)?;
        }
        Ok(())
    }

    fn publish_reasoning(&self, event: &RuntimeEvent) -> Result<()> {
        for update in self.turns.reasoning(
            &event.session_id,
            &event.data,
            event.kind == EventKind::Reasoning,
        )? {
            if update.persist {
                self.save_reasoning(&update.message)?;
            } else {
                self.emit(
                    &event.session_id,
                    "message",
                    serde_json::to_value(update.message)?,
                );
            }
        }
        Ok(())
    }

    fn save_reasoning(&self, message: &Message) -> Result<()> {
        let saved = self.store.save_reasoning(message)?;
        self.turns.reasoning_saved(&saved)?;
        self.emit(&saved.session_id, "message", serde_json::to_value(&saved)?);
        Ok(())
    }

    pub(crate) fn messages(&self, session_id: &str) -> Result<Vec<Message>> {
        let mut messages = self.store.messages(session_id)?;
        for live in self.turns.reasoning_messages(session_id)? {
            if let Some(message) = messages.iter_mut().find(|message| message.id == live.id) {
                *message = live;
            } else {
                messages.push(live);
            }
        }
        Ok(messages)
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
        self.save_finished_messages(self.turns.finish(id)?)
    }

    fn save_finished_messages(&self, messages: Vec<Message>) -> Result<()> {
        for message in messages {
            let message = match message.role {
                MessageRole::Activity => self.store.save_activity(&message)?,
                MessageRole::Reasoning => self.store.save_reasoning(&message)?,
                MessageRole::Agent => self.store.save_agent(&message)?,
                _ => self.store.append_message(&message)?,
            };
            self.emit(
                &message.session_id,
                "message",
                serde_json::to_value(&message)?,
            );
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

#[cfg(test)]
mod tests;
