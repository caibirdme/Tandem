use std::{collections::HashMap, sync::Mutex};

use serde_json::Value;
use uuid::Uuid;

use crate::{
    Error, Result,
    activities::Activities,
    app::now,
    model::{ActiveTurnView, Message, MessageRole},
    reasoning::{ReasoningUpdate, Reasonings},
    subagents::Subagents,
};

#[derive(Default)]
struct ActiveTurn {
    messages: Vec<(String, Message)>,
    settled_messages: Vec<(String, Message)>,
    permissions: Vec<Value>,
    activities: HashMap<Option<String>, Activities>,
    agents: Subagents,
    reasoning: Reasonings,
}

/// Only unfinished output and live approvals belong here; durable text lives in SQLite.
#[derive(Default)]
pub(crate) struct ActiveTurns(Mutex<HashMap<String, ActiveTurn>>);

impl ActiveTurns {
    pub fn reasoning(
        &self,
        session_id: &str,
        data: &Value,
        complete: bool,
    ) -> Result<Vec<ReasoningUpdate>> {
        let mut turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        Ok(turns
            .entry(session_id.to_owned())
            .or_default()
            .reasoning
            .update(session_id, data, complete))
    }

    pub fn close_reasoning(&self, session_id: &str, data: &Value) -> Result<Vec<Message>> {
        let mut turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        Ok(turns.get_mut(session_id).map_or_else(Vec::new, |turn| {
            turn.reasoning.close(
                data["agentId"].as_str(),
                crate::model::ActivityStatus::Completed,
            )
        }))
    }

    pub fn reasoning_saved(&self, message: &Message) -> Result<()> {
        let mut turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        if let Some(turn) = turns.get_mut(&message.session_id) {
            turn.reasoning.saved(message);
        }
        Ok(())
    }

    pub fn pause_reasoning(&self, session_id: &str) -> Result<Vec<Message>> {
        let mut turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        Ok(turns
            .get_mut(session_id)
            .map_or_else(Vec::new, |turn| turn.reasoning.pause()))
    }

    pub fn reasoning_messages(&self, session_id: &str) -> Result<Vec<Message>> {
        let turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        Ok(turns
            .get(session_id)
            .map_or_else(Vec::new, |turn| turn.reasoning.messages()))
    }

    pub fn delta(&self, session_id: &str, data: &Value) -> Result<()> {
        let Some(content) = data["content"]
            .as_str()
            .filter(|content| !content.is_empty())
        else {
            return Ok(());
        };
        let message_id = data["messageId"].as_str().unwrap_or_default();
        let agent_id = data["agentId"].as_str();
        let mut turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        let turn = turns.entry(session_id.to_owned()).or_default();
        if let Some((_, message)) = turn
            .messages
            .iter_mut()
            .find(|(id, message)| id == message_id && message.agent_id.as_deref() == agent_id)
        {
            message.content.push_str(content);
        } else {
            turn.messages.push((
                message_id.to_owned(),
                Message {
                    id: Uuid::new_v4().to_string(),
                    session_id: session_id.into(),
                    role: MessageRole::Assistant,
                    content: content.into(),
                    attachments: Vec::new(),
                    created_at: now(),
                    sequence: None,
                    activity: None,
                    reasoning: None,
                    selected_skills: Vec::new(),
                    agent_id: agent_id.map(str::to_owned),
                    agent: None,
                    fleet: false,
                    interrupted: true,
                },
            ));
        }
        Ok(())
    }

    pub fn checkpointed_message(&self, session_id: &str, data: &Value) -> Result<Option<Message>> {
        let turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        Ok(turns.get(session_id).and_then(|turn| {
            turn.settled_messages
                .iter()
                .find(|(id, message)| {
                    id == data["id"].as_str().unwrap_or_default()
                        && message.agent_id.as_deref() == data["agentId"].as_str()
                })
                .map(|(_, message)| message.clone())
        }))
    }

    pub fn finish_agent(&self, session_id: &str, data: &Value) -> Result<Vec<Message>> {
        let Some(agent_id) = data["agentId"].as_str() else {
            return Ok(Vec::new());
        };
        let mut turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        let Some(turn) = turns.get_mut(session_id) else {
            return Ok(Vec::new());
        };
        let mut finished = Vec::new();
        let mut remaining = Vec::new();
        for pending in turn.messages.drain(..) {
            if pending.1.agent_id.as_deref() == Some(agent_id) {
                finished.push(pending.1.clone());
                turn.settled_messages.push(pending);
            } else {
                remaining.push(pending);
            }
        }
        turn.messages = remaining;
        if let Some(activities) = turn.activities.get_mut(&Some(agent_id.to_owned())) {
            finished.extend(activities.settle());
        }
        let status = if data["event"] == "subagent.failed" || data["cancelled"] == true {
            crate::model::ActivityStatus::Interrupted
        } else {
            crate::model::ActivityStatus::Completed
        };
        finished.extend(turn.reasoning.close(Some(agent_id), status));
        Ok(finished)
    }

    pub fn completed(&self, session_id: &str, data: &Value) -> Result<()> {
        let mut turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        if let Some(turn) = turns.get_mut(session_id) {
            let id = data["id"].as_str().unwrap_or_default();
            let content = data["content"].as_str().unwrap_or_default();
            let agent_id = data["agentId"].as_str();
            turn.settled_messages.retain(|(pending_id, message)| {
                pending_id != id || message.agent_id.as_deref() != agent_id
            });
            turn.messages.retain(|(pending_id, message)| {
                if message.agent_id.as_deref() != agent_id {
                    return true;
                }
                if !id.is_empty() && !pending_id.is_empty() {
                    pending_id != id
                } else {
                    message.content != content
                }
            });
        }
        Ok(())
    }

    pub fn assistant(&self, message: Message, data: &Value) -> Result<()> {
        let mut turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        turns
            .entry(message.session_id.clone())
            .or_default()
            .activities
            .entry(message.agent_id.clone())
            .or_insert_with(|| Activities::new(message.agent_id.clone()))
            .assistant(message, data);
        Ok(())
    }

    pub fn intent(&self, session_id: &str, data: &Value) -> Result<()> {
        let mut turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        turns
            .entry(session_id.to_owned())
            .or_default()
            .activities
            .entry(data["agentId"].as_str().map(str::to_owned))
            .or_insert_with(|| Activities::new(data["agentId"].as_str().map(str::to_owned)))
            .intent(data);
        Ok(())
    }

    pub fn tool(&self, session_id: &str, data: &Value) -> Result<Option<Message>> {
        let mut turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        let turn = turns.entry(session_id.to_owned()).or_default();
        turn.agents.task(data);
        Ok(turn
            .activities
            .entry(data["agentId"].as_str().map(str::to_owned))
            .or_insert_with(|| Activities::new(data["agentId"].as_str().map(str::to_owned)))
            .tool(session_id, data))
    }

    pub fn agent(&self, session_id: &str, data: &Value) -> Result<Option<Message>> {
        let mut turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        let turn = if data["event"] == "subagent.started" {
            turns.entry(session_id.to_owned()).or_default()
        } else if let Some(turn) = turns.get_mut(session_id) {
            turn
        } else {
            return Ok(None);
        };
        Ok(turn.agents.update(session_id, data))
    }

    pub fn agent_saved(&self, message: &Message) -> Result<()> {
        let mut turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        if let Some(turn) = turns.get_mut(&message.session_id) {
            turn.agents.saved(message);
        }
        Ok(())
    }

    pub fn permission(&self, session_id: &str, data: &Value) -> Result<()> {
        let mut turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        let permissions = &mut turns.entry(session_id.to_owned()).or_default().permissions;
        permissions.retain(|permission| permission["requestId"] != data["requestId"]);
        permissions.push(data.clone());
        Ok(())
    }

    pub fn resolve_permission(&self, session_id: &str, request_id: &str) -> Result<()> {
        let mut turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        if let Some(turn) = turns.get_mut(session_id) {
            turn.permissions
                .retain(|permission| permission["requestId"] != request_id);
        }
        Ok(())
    }

    pub fn has_pending_permissions(&self, session_id: &str) -> Result<bool> {
        let turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        Ok(turns
            .get(session_id)
            .is_some_and(|turn| !turn.permissions.is_empty()))
    }

    pub fn finish(&self, session_id: &str) -> Result<Vec<Message>> {
        let mut turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        Ok(turns.remove(session_id).map_or_else(Vec::new, |turn| {
            turn.messages
                .into_iter()
                .map(|(_, message)| message)
                .chain(turn.activities.into_values().flat_map(Activities::finish))
                .chain(turn.agents.finish())
                .chain(turn.reasoning.finish())
                .collect()
        }))
    }

    pub fn snapshot(&self) -> Result<HashMap<String, ActiveTurnView>> {
        let turns = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        Ok(turns
            .iter()
            .map(|(id, turn)| {
                (
                    id.clone(),
                    ActiveTurnView {
                        stream: turn
                            .messages
                            .iter()
                            .filter(|(_, message)| message.agent_id.is_none())
                            .map(|(_, message)| message.content.as_str())
                            .collect(),
                        agent_streams: turn.messages.iter().fold(
                            HashMap::new(),
                            |mut streams, (_, message)| {
                                if let Some(agent_id) = &message.agent_id {
                                    streams
                                        .entry(agent_id.clone())
                                        .or_insert_with(String::new)
                                        .push_str(&message.content);
                                }
                                streams
                            },
                        ),
                        permissions: turn.permissions.clone(),
                    },
                )
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use serde_json::json;

    use super::*;

    #[test]
    fn completed_messages_are_not_persisted_again_when_a_turn_finishes() -> Result<()> {
        for completed in [true, false] {
            let turns = ActiveTurns::default();
            for content in ["partial ", "reply"] {
                turns.delta("session", &json!({"messageId":"message","content":content}))?;
            }
            assert_eq!(turns.snapshot()?["session"].stream, "partial reply");
            if completed {
                turns.completed(
                    "session",
                    &json!({"id":"message","content":"partial reply"}),
                )?;
            }
            let pending = turns.finish("session")?;
            assert_eq!(pending.len(), usize::from(!completed));
            if let Some(message) = pending.first() {
                assert!(message.interrupted);
                assert_eq!(message.content, "partial reply");
            }
            assert!(turns.finish("session")?.is_empty());
            assert!(turns.snapshot()?.is_empty());
        }
        Ok(())
    }

    #[test]
    fn snapshots_restore_live_approvals_without_reusing_resolved_requests() -> Result<()> {
        let turns = ActiveTurns::default();
        for id in ["first", "second", "first"] {
            turns.permission(
                "session",
                &json!({"requestId":id,"description":"Write file"}),
            )?;
        }
        assert_eq!(turns.snapshot()?["session"].permissions.len(), 2);
        turns.resolve_permission("session", "first")?;
        assert_eq!(
            turns.snapshot()?["session"].permissions[0]["requestId"],
            "second"
        );
        turns.finish("session")?;
        assert!(turns.snapshot()?.is_empty());
        Ok(())
    }

    #[test]
    fn completion_uses_message_identity_before_matching_text() -> Result<()> {
        for (pending_id, completed_id, remaining) in [
            ("first", "first", 0),
            ("second", "first", 1),
            ("", "first", 0),
            ("first", "", 0),
        ] {
            let turns = ActiveTurns::default();
            turns.delta(
                "session",
                &json!({"messageId":pending_id,"content":"same text"}),
            )?;
            turns.completed("session", &json!({"id":completed_id,"content":"same text"}))?;
            assert_eq!(
                turns.finish("session")?.len(),
                remaining,
                "{pending_id}/{completed_id}"
            );
        }
        Ok(())
    }
}
