use std::collections::{HashMap, HashSet};

use serde_json::Value;
use uuid::Uuid;

use crate::{
    app::now,
    model::{Activity, ActivityStatus, Message, MessageRole, ToolActivity},
};

struct ProgressMessage {
    message: Message,
    calls: HashSet<String>,
    turn_id: Option<String>,
}

#[derive(Default)]
pub(crate) struct Activities {
    messages: Vec<Message>,
    current: Option<usize>,
    intent: Option<String>,
    progress: Option<ProgressMessage>,
    hidden_calls: HashSet<String>,
    owners: HashMap<String, usize>,
}

impl Activities {
    pub fn intent(&mut self, data: &Value) {
        if let Some(intent) = data["intent"]
            .as_str()
            .filter(|text| !text.trim().is_empty())
        {
            self.intent = Some(intent.to_owned());
        }
    }

    pub fn assistant(&mut self, message: Message, data: &Value) {
        self.progress = Some(ProgressMessage {
            message,
            calls: data["toolRequests"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|request| !request["name"].as_str().is_some_and(internal_tool))
                .filter_map(|request| request["toolCallId"].as_str().map(str::to_owned))
                .collect(),
            turn_id: data["turnId"].as_str().map(str::to_owned),
        });
    }

    pub fn tool(&mut self, session_id: &str, data: &Value) -> Option<Message> {
        if data["event"] == "tool.execution_partial_result" {
            return None;
        }
        let details = &data["details"];
        let call_id = details["toolCallId"].as_str().filter(|id| !id.is_empty())?;
        let name = details["toolName"].as_str();
        if name.is_some_and(internal_tool) {
            self.hidden_calls.insert(call_id.to_owned());
        }
        if self.hidden_calls.contains(call_id) {
            return None;
        }

        let owner = self
            .owners
            .get(call_id)
            .copied()
            .unwrap_or_else(|| self.start_for_tool(session_id, call_id, data));
        self.owners.insert(call_id.into(), owner);
        let message = &mut self.messages[owner];
        let activity = message.activity.as_mut()?;
        let index = activity
            .tools
            .iter()
            .position(|tool| tool.tool_call_id == call_id);
        let tool = if let Some(index) = index {
            &mut activity.tools[index]
        } else {
            activity.tools.push(ToolActivity {
                tool_call_id: call_id.into(),
                name: name.unwrap_or("tool").into(),
                title: None,
                status: ActivityStatus::Running,
                arguments: None,
                result: None,
                error: None,
                progress: None,
            });
            activity.tools.last_mut()?
        };
        update_tool(tool, data);
        activity.status = activity_status(&activity.tools);
        Some(message.clone())
    }

    fn start_for_tool(&mut self, session_id: &str, call_id: &str, data: &Value) -> usize {
        let progress_matches = data["event"] == "tool.execution_start"
            && self.progress.as_ref().is_some_and(|progress| {
                progress.calls.contains(call_id)
                    || (progress.calls.is_empty()
                        && match (
                            progress.turn_id.as_deref(),
                            data["details"]["turnId"].as_str(),
                        ) {
                            (Some(previous), Some(current)) => previous == current,
                            _ => true,
                        })
            });
        let progress = progress_matches.then(|| self.progress.take()).flatten();
        let intent = self.intent.take();
        if let Some(progress) = progress {
            let mut message = progress.message;
            message.role = MessageRole::Activity;
            message.activity = Some(Activity {
                status: ActivityStatus::Running,
                tools: Vec::new(),
            });
            self.messages.push(message);
            let index = self.messages.len() - 1;
            self.current = Some(index);
            for call in progress.calls {
                self.owners.insert(call, index);
            }
        } else if let Some(intent) = intent
            && self
                .current
                .is_none_or(|index| self.messages[index].content != intent)
        {
            self.start(session_id, intent);
        }
        self.current
            .unwrap_or_else(|| self.start(session_id, String::new()))
    }

    fn start(&mut self, session_id: &str, content: String) -> usize {
        self.messages.push(Message {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.into(),
            role: MessageRole::Activity,
            content,
            attachments: Vec::new(),
            created_at: now(),
            sequence: None,
            activity: Some(Activity {
                status: ActivityStatus::Running,
                tools: Vec::new(),
            }),
            reasoning: None,
            selected_skills: Vec::new(),
            interrupted: false,
        });
        let index = self.messages.len() - 1;
        self.current = Some(index);
        index
    }

    pub fn finish(self) -> Vec<Message> {
        self.messages
            .into_iter()
            .filter_map(|mut message| {
                let activity = message.activity.as_mut()?;
                let changed = interrupt_activity(activity);
                changed.then_some(message)
            })
            .collect()
    }
}

fn internal_tool(name: &str) -> bool {
    matches!(
        name.strip_prefix("builtin:").unwrap_or(name),
        "report_intent" | "task_complete"
    )
}

fn update_tool(tool: &mut ToolActivity, data: &Value) {
    let details = &data["details"];
    let event = data["event"].as_str().unwrap_or_default();
    if let Some(name) = details["toolName"].as_str() {
        tool.name = name.into();
    }
    if let Some(title) = details["toolTitle"]
        .as_str()
        .filter(|title| !title.is_empty())
    {
        tool.title = Some(title.into());
    }
    for (target, field) in [
        (&mut tool.arguments, "arguments"),
        (&mut tool.result, "result"),
        (&mut tool.error, "error"),
    ] {
        if let Some(value) = details.get(field).filter(|value| !value.is_null()) {
            *target = Some(value.clone());
        }
    }
    if let Some(progress) = details["progressMessage"].as_str() {
        tool.progress = Some(progress.into());
    }
    if event == "tool.shell_output"
        && let Some(text) = details["text"].as_str()
    {
        let output = tool
            .result
            .get_or_insert_with(|| Value::String(String::new()));
        if let Some(previous) = output.as_str() {
            *output = Value::String(format!("{previous}{text}"));
        }
    }
    if details["success"] == false || tool.error.is_some() {
        tool.status = ActivityStatus::Error;
    } else if event == "tool.execution_complete" {
        tool.status = ActivityStatus::Completed;
    }
}

fn activity_status(tools: &[ToolActivity]) -> ActivityStatus {
    [
        ActivityStatus::Running,
        ActivityStatus::Error,
        ActivityStatus::Interrupted,
    ]
    .into_iter()
    .find(|status| tools.iter().any(|tool| tool.status == *status))
    .unwrap_or(ActivityStatus::Completed)
}

pub(crate) fn interrupt_activity(activity: &mut Activity) -> bool {
    let mut changed = false;
    for tool in &mut activity.tools {
        if tool.status == ActivityStatus::Running {
            tool.status = ActivityStatus::Interrupted;
            changed = true;
        }
    }
    if changed || activity.status == ActivityStatus::Running {
        activity.status = activity_status(&activity.tools);
        true
    } else {
        false
    }
}
