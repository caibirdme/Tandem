use serde_json::Value;
use uuid::Uuid;

use crate::{
    app::now,
    model::{ActivityStatus, Message, MessageRole, Reasoning},
};

struct Block {
    reasoning_id: String,
    agent_id: Option<String>,
    message: Message,
}

pub(crate) struct ReasoningUpdate {
    pub message: Message,
    pub persist: bool,
}

/// Streamed text stays in memory between the first durable chunk and block completion.
#[derive(Default)]
pub(crate) struct Reasonings(Vec<Block>);

impl Reasonings {
    pub fn update(
        &mut self,
        session_id: &str,
        data: &Value,
        complete: bool,
    ) -> Vec<ReasoningUpdate> {
        let Some(reasoning_id) = data["reasoningId"].as_str().filter(|id| !id.is_empty()) else {
            return Vec::new();
        };
        let Some(content) = data["content"].as_str() else {
            return Vec::new();
        };
        if !complete && content.is_empty() {
            return Vec::new();
        }
        let agent_id = data["agentId"].as_str();
        if let Some(block) = self.0.iter_mut().find(|block| {
            block.reasoning_id == reasoning_id && block.agent_id.as_deref() == agent_id
        }) {
            if complete {
                if !content.is_empty() {
                    block.message.content = content.into();
                }
                block.message.reasoning = Some(Reasoning {
                    status: ActivityStatus::Completed,
                });
            } else {
                block.message.content.push_str(content);
            }
            return vec![ReasoningUpdate {
                message: block.message.clone(),
                persist: complete || !running(&block.message),
            }];
        }

        if content.is_empty() {
            return Vec::new();
        }

        let mut updates: Vec<_> = self
            .close(agent_id, ActivityStatus::Completed)
            .into_iter()
            .map(|message| ReasoningUpdate {
                message,
                persist: true,
            })
            .collect();
        let message = Message {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.into(),
            role: MessageRole::Reasoning,
            content: content.into(),
            attachments: Vec::new(),
            selected_skills: Vec::new(),
            created_at: now(),
            sequence: None,
            activity: None,
            reasoning: Some(Reasoning {
                status: if complete {
                    ActivityStatus::Completed
                } else {
                    ActivityStatus::Running
                },
            }),
            agent_id: agent_id.map(str::to_owned),
            agent: None,
            fleet: false,
            interrupted: false,
        };
        self.0.push(Block {
            reasoning_id: reasoning_id.into(),
            agent_id: agent_id.map(str::to_owned),
            message: message.clone(),
        });
        updates.push(ReasoningUpdate {
            message,
            persist: true,
        });
        updates
    }

    pub fn close(&mut self, agent_id: Option<&str>, status: ActivityStatus) -> Vec<Message> {
        self.0
            .iter_mut()
            .filter(|block| block.agent_id.as_deref() == agent_id)
            .filter_map(|block| close(&mut block.message, status))
            .collect()
    }

    pub fn saved(&mut self, message: &Message) {
        if let Some(block) = self
            .0
            .iter_mut()
            .find(|block| block.message.id == message.id)
        {
            block.message.sequence = message.sequence;
        }
    }

    pub fn pause(&mut self) -> Vec<Message> {
        self.0
            .iter_mut()
            .filter_map(|block| close(&mut block.message, ActivityStatus::Completed))
            .collect()
    }

    pub fn messages(&self) -> Vec<Message> {
        self.0.iter().map(|block| block.message.clone()).collect()
    }

    pub fn finish(self) -> Vec<Message> {
        self.0
            .into_iter()
            .filter_map(|mut block| close(&mut block.message, ActivityStatus::Interrupted))
            .collect()
    }
}

fn running(message: &Message) -> bool {
    message
        .reasoning
        .as_ref()
        .is_some_and(|reasoning| reasoning.status == ActivityStatus::Running)
}

fn close(message: &mut Message, status: ActivityStatus) -> Option<Message> {
    if !running(message) {
        return None;
    }
    message.reasoning = Some(Reasoning { status });
    message.interrupted = status == ActivityStatus::Interrupted;
    Some(message.clone())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn streaming_reuses_identity_and_only_checkpoints_first_and_complete_text() {
        let mut reasoning = Reasonings::default();
        let mut first = reasoning.update(
            "session-1",
            &json!({"reasoningId": "reasoning-1", "content": "Inspecting"}),
            false,
        );
        assert!(first[0].persist);
        first[0].message.sequence = Some(7);
        reasoning.saved(&first[0].message);
        let next = reasoning.update(
            "session-1",
            &json!({"reasoningId": "reasoning-1", "content": " files"}),
            false,
        );
        assert!(!next[0].persist);
        assert_eq!(next[0].message.id, first[0].message.id);
        assert_eq!(next[0].message.sequence, Some(7));
        assert_eq!(reasoning.messages()[0].content, "Inspecting files");
        let complete = reasoning.update(
            "session-1",
            &json!({"reasoningId": "reasoning-1", "content": "Inspected the files"}),
            true,
        );
        assert!(complete[0].persist);
        assert_eq!(complete[0].message.id, first[0].message.id);
        assert_eq!(complete[0].message.content, "Inspected the files");
        assert!(!running(&complete[0].message));
        assert!(reasoning.finish().is_empty());
    }

    #[test]
    fn reasoning_lifecycle_keeps_agents_separate_and_interrupts_unfinished_blocks() {
        for pause in [false, true] {
            let mut reasoning = Reasonings::default();
            reasoning.update(
                "session-1",
                &json!({"reasoningId": "shared-id", "content": "Main agent"}),
                false,
            );
            reasoning.update(
                "session-1",
                &json!({"reasoningId": "shared-id", "agentId": "child", "content": "Child agent"}),
                false,
            );
            assert_eq!(reasoning.messages().len(), 2);
            assert_eq!(reasoning.close(None, ActivityStatus::Completed).len(), 1);
            if pause {
                assert_eq!(reasoning.pause().len(), 1);
                assert!(reasoning.finish().is_empty());
            } else {
                let interrupted = reasoning.finish();
                assert_eq!(interrupted.len(), 1);
                assert_eq!(interrupted[0].content, "Child agent");
                assert!(interrupted[0].interrupted);
                assert_eq!(
                    interrupted[0].reasoning.as_ref().map(|state| state.status),
                    Some(ActivityStatus::Interrupted),
                );
            }
        }
    }
}
