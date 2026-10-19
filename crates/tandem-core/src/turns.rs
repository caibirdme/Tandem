use std::{collections::HashMap, sync::Mutex};

use anyhow::{Result, anyhow};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    app::now,
    model::{ActiveTurnView, Message, MessageRole},
};

#[derive(Default)]
struct ActiveTurn {
    messages: Vec<(String, Message)>,
    permissions: Vec<Value>,
}

/// Only unfinished output and live approvals belong here; durable text lives in SQLite.
#[derive(Default)]
pub(crate) struct ActiveTurns(Mutex<HashMap<String, ActiveTurn>>);

impl ActiveTurns {
    pub fn delta(&self, session_id: &str, data: &Value) -> Result<()> {
        let Some(content) = data["content"]
            .as_str()
            .filter(|content| !content.is_empty())
        else {
            return Ok(());
        };
        let message_id = data["messageId"].as_str().unwrap_or_default();
        let mut turns = self.0.lock().map_err(|_| anyhow!("会话状态锁已损坏"))?;
        let turn = turns.entry(session_id.to_owned()).or_default();
        if let Some((_, message)) = turn.messages.iter_mut().find(|(id, _)| id == message_id) {
            message.content.push_str(content);
        } else {
            turn.messages.push((
                message_id.to_owned(),
                Message {
                    id: Uuid::new_v4().to_string(),
                    session_id: session_id.into(),
                    role: MessageRole::Assistant,
                    content: content.into(),
                    created_at: now(),
                    interrupted: true,
                },
            ));
        }
        Ok(())
    }

    pub fn completed(&self, session_id: &str, data: &Value) -> Result<()> {
        let mut turns = self.0.lock().map_err(|_| anyhow!("会话状态锁已损坏"))?;
        if let Some(turn) = turns.get_mut(session_id) {
            let id = data["id"].as_str().unwrap_or_default();
            let content = data["content"].as_str().unwrap_or_default();
            turn.messages.retain(|(pending_id, message)| {
                if !id.is_empty() && !pending_id.is_empty() {
                    pending_id != id
                } else {
                    message.content != content
                }
            });
        }
        Ok(())
    }

    pub fn permission(&self, session_id: &str, data: &Value) -> Result<()> {
        let mut turns = self.0.lock().map_err(|_| anyhow!("会话状态锁已损坏"))?;
        let permissions = &mut turns.entry(session_id.to_owned()).or_default().permissions;
        permissions.retain(|permission| permission["requestId"] != data["requestId"]);
        permissions.push(data.clone());
        Ok(())
    }

    pub fn resolve_permission(&self, session_id: &str, request_id: &str) -> Result<()> {
        let mut turns = self.0.lock().map_err(|_| anyhow!("会话状态锁已损坏"))?;
        if let Some(turn) = turns.get_mut(session_id) {
            turn.permissions
                .retain(|permission| permission["requestId"] != request_id);
        }
        Ok(())
    }

    pub fn has_pending_permissions(&self, session_id: &str) -> Result<bool> {
        let turns = self.0.lock().map_err(|_| anyhow!("会话状态锁已损坏"))?;
        Ok(turns
            .get(session_id)
            .is_some_and(|turn| !turn.permissions.is_empty()))
    }

    pub fn finish(&self, session_id: &str) -> Result<Vec<Message>> {
        let mut turns = self.0.lock().map_err(|_| anyhow!("会话状态锁已损坏"))?;
        Ok(turns.remove(session_id).map_or_else(Vec::new, |turn| {
            turn.messages
                .into_iter()
                .map(|(_, message)| message)
                .collect()
        }))
    }

    pub fn snapshot(&self) -> Result<HashMap<String, ActiveTurnView>> {
        let turns = self.0.lock().map_err(|_| anyhow!("会话状态锁已损坏"))?;
        Ok(turns
            .iter()
            .map(|(id, turn)| {
                (
                    id.clone(),
                    ActiveTurnView {
                        stream: turn
                            .messages
                            .iter()
                            .map(|(_, message)| message.content.as_str())
                            .collect(),
                        permissions: turn.permissions.clone(),
                    },
                )
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
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
