use std::collections::HashMap;

use serde_json::Value;
use uuid::Uuid;

use crate::{
    app::now,
    model::{ActivityStatus, Message, MessageRole, Subagent},
};

/// Lifecycle messages remain in the parent transcript; worker output is scoped by ID.
#[derive(Default)]
pub(crate) struct Subagents {
    messages: Vec<Message>,
    assignments: HashMap<String, String>,
}

impl Subagents {
    pub fn task(&mut self, data: &Value) {
        let details = &data["details"];
        if data["event"] != "tool.execution_start"
            || details["toolName"]
                .as_str()
                .is_none_or(|name| name.strip_prefix("builtin:").unwrap_or(name) != "task")
        {
            return;
        }
        if let (Some(call_id), Some(description)) = (
            details["toolCallId"].as_str(),
            ["description", "prompt"].iter().find_map(|field| {
                details["arguments"][field]
                    .as_str()
                    .filter(|description| !description.trim().is_empty())
            }),
        ) {
            self.assignments.insert(
                call_id.into(),
                description
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .chars()
                    .take(160)
                    .collect(),
            );
        }
    }

    pub fn update(&mut self, session_id: &str, data: &Value) -> Option<Message> {
        let status = match data["event"].as_str()? {
            "subagent.started" => ActivityStatus::Running,
            "subagent.completed" if data["cancelled"] == true => ActivityStatus::Interrupted,
            "subagent.completed" => ActivityStatus::Completed,
            "subagent.failed" => ActivityStatus::Error,
            _ => return None,
        };
        let call_id = data["toolCallId"].as_str().filter(|id| !id.is_empty())?;
        let id = data["agentId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .unwrap_or(call_id);
        let index = self.messages.iter().position(|message| {
            message
                .agent
                .as_ref()
                .is_some_and(|agent| agent.id == id || agent.tool_call_id == call_id)
        });
        let index = index.or_else(|| {
            // Terminal notifications can arrive after a stopped turn is settled.
            // Only a start event may introduce a worker into the current turn.
            if status != ActivityStatus::Running {
                return None;
            }
            let name = data["agentName"].as_str().unwrap_or("agent");
            self.messages.push(Message {
                id: Uuid::new_v4().to_string(),
                session_id: session_id.into(),
                role: MessageRole::Agent,
                agent_id: None,
                agent: Some(Subagent {
                    id: id.into(),
                    tool_call_id: call_id.into(),
                    name: name.into(),
                    display_name: data["agentDisplayName"].as_str().unwrap_or(name).into(),
                    description: data["agentDescription"].as_str().unwrap_or_default().into(),
                    status,
                    model: None,
                    error: None,
                    parent_agent_id: data["parentId"].as_str().map(str::to_owned),
                }),
                content: String::new(),
                attachments: Vec::new(),
                selected_skills: Vec::new(),
                created_at: now(),
                sequence: None,
                activity: None,
                reasoning: None,
                fleet: false,
                interrupted: false,
            });
            Some(self.messages.len() - 1)
        })?;
        let message = &mut self.messages[index];
        let agent = message.agent.as_mut()?;
        if data["agentId"].as_str().is_some_and(|id| !id.is_empty()) {
            agent.id = id.into();
        }
        if status != ActivityStatus::Running || agent.status == ActivityStatus::Running {
            agent.status = status;
        }
        message.interrupted = agent.status == ActivityStatus::Interrupted;
        for (target, field) in [
            (&mut agent.name, "agentName"),
            (&mut agent.display_name, "agentDisplayName"),
            (&mut agent.description, "agentDescription"),
        ] {
            if let Some(value) = data[field].as_str().filter(|text| !text.is_empty()) {
                *target = value.into();
            }
        }
        if let Some(assignment) = self.assignments.get(call_id) {
            agent.display_name.clone_from(assignment);
        }
        if let Some(model) = data["model"].as_str() {
            agent.model = Some(model.into());
        }
        if let Some(error) = data["error"].as_str() {
            agent.error = Some(error.into());
        }
        Some(message.clone())
    }

    pub fn saved(&mut self, message: &Message) {
        if let Some(current) = self
            .messages
            .iter_mut()
            .find(|current| current.id == message.id)
        {
            current.sequence = message.sequence;
        }
    }

    pub fn finish(self) -> Vec<Message> {
        self.messages
            .into_iter()
            .filter_map(|mut message| {
                let agent = message.agent.as_mut()?;
                if agent.status != ActivityStatus::Running {
                    return None;
                }
                agent.status = ActivityStatus::Interrupted;
                message.interrupted = true;
                Some(message)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use anyhow::{Context, Result};
    use serde_json::json;

    use super::*;

    #[test]
    fn assignment_titles_identify_workers_and_terminal_events_keep_stable_identity() -> Result<()> {
        let mut agents = Subagents::default();
        agents.task(&json!({
            "event":"tool.execution_start",
            "details":{"toolCallId":"dispatch","toolName":"task","arguments":{"description":"Inspect permissions"}}
        }));
        let first = agents.update("session", &json!({
            "event":"subagent.started", "toolCallId":"dispatch", "agentId":"worker",
            "agentName":"general-purpose", "agentDisplayName":"General purpose", "parentId":"parent"
        })).context("valid lifecycle event")?;
        let agent = first.agent.as_ref().context("agent metadata")?;
        assert_eq!(agent.display_name, "Inspect permissions");
        assert_eq!(agent.name, "general-purpose");
        assert_eq!(agent.parent_agent_id.as_deref(), Some("parent"));
        let completed = agents
            .update(
                "session",
                &json!({
                    "event":"subagent.failed", "toolCallId":"dispatch", "error":"Permission denied"
                }),
            )
            .context("valid lifecycle event")?;
        assert_eq!(completed.id, first.id);
        let agent = completed.agent.as_ref().context("agent metadata")?;
        assert_eq!(agent.id, "worker");
        assert_eq!(agent.status, ActivityStatus::Error);
        assert_eq!(agent.error.as_deref(), Some("Permission denied"));
        assert!(agents.finish().is_empty());
        Ok(())
    }
    #[test]
    fn cancelled_workers_are_interrupted_instead_of_successful() -> Result<()> {
        let mut agents = Subagents::default();
        for cancelled in [true, false] {
            assert!(
                agents
                    .update(
                        "session",
                        &json!({
                            "event":"subagent.started", "toolCallId":cancelled.to_string(),
                            "agentId":cancelled.to_string()
                        }),
                    )
                    .is_some()
            );
            let message = agents
                .update(
                    "session",
                    &json!({
                        "event":"subagent.completed", "toolCallId":cancelled.to_string(),
                        "agentId":cancelled.to_string(), "cancelled":cancelled
                    }),
                )
                .context("valid completion event")?;
            assert_eq!(message.interrupted, cancelled);
            assert_eq!(
                message.agent.context("agent metadata")?.status,
                if cancelled {
                    ActivityStatus::Interrupted
                } else {
                    ActivityStatus::Completed
                }
            );
        }
        Ok(())
    }
}
