use anyhow::{Context, Result};
use tempfile::TempDir;

use super::*;
use crate::{AppOptions, model::PermissionMode};

async fn application() -> Result<(TempDir, Arc<App>, String)> {
    let directory = TempDir::new()?;
    let app = App::open(AppOptions::local(directory.path().into()))?;
    let provider = app.save_provider(serde_json::from_value(json!({
        "id":"", "name":"Fixture", "protocol":"anthropic", "baseUrl":"https://example.invalid",
        "models":[{"id":"fixture","name":"Fixture","contextWindow":200_000}]
    }))?, None).await?;
    let session = app
        .create_session(
            None,
            provider.id,
            "fixture".into(),
            Some(PermissionMode::Ask),
            None,
        )
        .await?;
    Ok((directory, app, session.id))
}

fn receive(app: &Arc<App>, session_id: &str, kind: EventKind, data: Value) {
    app.receive_event(&RuntimeEvent {
        session_id: session_id.into(),
        kind,
        data,
    });
}

fn tool(app: &Arc<App>, session_id: &str, event: &str, details: Value) {
    let mut data = json!({"event":event});
    data["details"] = details;
    receive(app, session_id, EventKind::Tool, data);
}

#[tokio::test]
async fn reasoning_stream_restores_live_text_and_persists_one_completed_message() -> Result<()> {
    let (directory, app, id) = application().await?;
    for content in ["Inspecting ", "the files"] {
        receive(
            &app,
            &id,
            EventKind::ReasoningDelta,
            json!({"reasoningId":"reasoning-1","content":content}),
        );
    }
    let checkpoint = app.store.messages(&id)?;
    let live = app.messages(&id)?;
    assert_eq!(checkpoint.len(), 1);
    assert_eq!(checkpoint[0].content, "Inspecting ");
    assert_eq!(live[0].content, "Inspecting the files");
    assert_eq!(live[0].sequence, checkpoint[0].sequence);
    receive(
        &app,
        &id,
        EventKind::Intent,
        json!({"intent":"Read the source"}),
    );
    let completed = app.store.messages(&id)?;
    assert_eq!(completed[0].id, checkpoint[0].id);
    assert_eq!(completed[0].content, "Inspecting the files");
    assert_eq!(
        completed[0].reasoning.as_ref().context("reasoning")?.status,
        crate::model::ActivityStatus::Completed,
    );
    receive(
        &app,
        &id,
        EventKind::Reasoning,
        json!({"reasoningId":"reasoning-1","content":"Inspected the files"}),
    );
    receive(&app, &id, EventKind::Status, json!({"state":"idle"}));
    assert_eq!(app.store.messages(&id)?.len(), 1);
    drop(app);
    let reopened = App::open(AppOptions::local(directory.path().into()))?;
    let saved = reopened.store.messages(&id)?;
    assert_eq!(saved[0].id, checkpoint[0].id);
    assert_eq!(saved[0].content, "Inspected the files");
    Ok(())
}

#[tokio::test]
async fn earlier_tool_updates_do_not_finish_streaming_reasoning() -> Result<()> {
    let (_directory, app, id) = application().await?;
    tool(
        &app,
        &id,
        "tool.execution_start",
        json!({"toolCallId":"earlier-tool","toolName":"bash"}),
    );
    receive(
        &app,
        &id,
        EventKind::ReasoningDelta,
        json!({"reasoningId":"reasoning-1","content":"Inspecting"}),
    );
    for event in [
        "tool.execution_partial_result",
        "tool.execution_progress",
        "tool.shell_output",
        "tool.execution_complete",
    ] {
        tool(
            &app,
            &id,
            event,
            json!({"toolCallId":"earlier-tool","progressMessage":"Running","text":"output","success":true}),
        );
        receive(
            &app,
            &id,
            EventKind::ReasoningDelta,
            json!({"reasoningId":"reasoning-1","content":" more"}),
        );
        let checkpoint = app.store.messages(&id)?;
        assert_eq!(checkpoint[1].content, "Inspecting");
        let live = app.messages(&id)?;
        assert_eq!(
            live[1].reasoning.as_ref().context("reasoning")?.status,
            crate::model::ActivityStatus::Running,
        );
    }
    tool(
        &app,
        &id,
        "tool.execution_start",
        json!({"toolCallId":"next-tool","toolName":"view"}),
    );
    let completed = app.store.messages(&id)?;
    assert_eq!(completed[1].content, "Inspecting more more more more");
    assert_eq!(
        completed[1].reasoning.as_ref().context("reasoning")?.status,
        crate::model::ActivityStatus::Completed,
    );
    Ok(())
}

#[tokio::test]
async fn progress_becomes_an_ordered_group_and_parallel_completions_keep_their_owner() -> Result<()>
{
    let (_directory, app, id) = application().await?;
    let mut events = app.subscribe();
    receive(
        &app,
        &id,
        EventKind::Message,
        json!({
            "id":"progress", "content":"Inspect the project", "turnId":"1",
            "toolRequests":[{"toolCallId":"a"},{"toolCallId":"b"}]
        }),
    );
    let initial = app.store.messages(&id)?[0].clone();
    tool(
        &app,
        &id,
        "tool.execution_start",
        json!({"toolCallId":"a","toolName":"view","arguments":{"path":"one.rs"}}),
    );
    receive(
        &app,
        &id,
        EventKind::Intent,
        json!({"intent":"Run verification"}),
    );
    tool(
        &app,
        &id,
        "tool.execution_start",
        json!({"toolCallId":"c","toolName":"bash"}),
    );
    tool(
        &app,
        &id,
        "tool.execution_start",
        json!({"toolCallId":"b","toolName":"view","arguments":{"path":"two.rs"}}),
    );
    tool(
        &app,
        &id,
        "tool.execution_progress",
        json!({"toolCallId":"a","progressMessage":"Reading"}),
    );
    tool(
        &app,
        &id,
        "tool.execution_complete",
        json!({"toolCallId":"a","success":true,"result":"first file"}),
    );
    tool(
        &app,
        &id,
        "tool.execution_complete",
        json!({"toolCallId":"b","success":true,"result":"second file"}),
    );
    tool(
        &app,
        &id,
        "tool.execution_complete",
        json!({"toolCallId":"c","success":false,"error":{"message":"failed"}}),
    );
    receive(
        &app,
        &id,
        EventKind::Message,
        json!({"id":"final","content":"The result is ready."}),
    );
    receive(&app, &id, EventKind::Status, json!({"state":"idle"}));

    let messages = app.store.messages(&id)?;
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].id, initial.id);
    assert_eq!(messages[0].sequence, initial.sequence);
    assert_eq!(messages[0].created_at, initial.created_at);
    assert_eq!(messages[0].role, MessageRole::Activity);
    let activity = messages[0].activity.as_ref().context("activity")?;
    assert_eq!(activity.tools.len(), 2);
    assert_eq!(activity.tools[0].name, "view");
    assert_eq!(activity.tools[0].arguments, Some(json!({"path":"one.rs"})));
    assert_eq!(activity.tools[0].result, Some(json!("first file")));
    assert_eq!(activity.tools[0].progress.as_deref(), Some("Reading"));
    assert_eq!(activity.status, crate::model::ActivityStatus::Completed);
    assert_eq!(messages[1].content, "Run verification");
    assert_eq!(
        messages[1].activity.as_ref().context("activity")?.status,
        crate::model::ActivityStatus::Error
    );
    assert_eq!(messages[2].role, MessageRole::Assistant);
    assert!(
        messages
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    while let Ok(event) = events.try_recv() {
        assert_ne!(event.kind, "tool");
    }
    Ok(())
}

#[tokio::test]
async fn tools_without_new_progress_share_a_group_and_history_survives_reopening() -> Result<()> {
    let (directory, app, id) = application().await?;
    for (call, turn) in [("a", "1"), ("b", "2")] {
        receive(
            &app,
            &id,
            EventKind::Status,
            json!({"state":"running","turnId":turn}),
        );
        tool(
            &app,
            &id,
            "tool.execution_start",
            json!({"toolCallId":call,"toolName":"bash"}),
        );
        tool(
            &app,
            &id,
            "tool.execution_complete",
            json!({"toolCallId":call,"success":true,"result":"done"}),
        );
    }
    receive(&app, &id, EventKind::Status, json!({"state":"idle"}));
    let history = app.store.messages(&id)?;
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0]
            .activity
            .as_ref()
            .context("activity")?
            .tools
            .len(),
        2
    );
    let reopened = App::open(AppOptions::local(directory.path().into()))?;
    assert_eq!(
        serde_json::to_value(history)?,
        serde_json::to_value(reopened.store.messages(&id)?)?
    );
    Ok(())
}

#[tokio::test]
async fn terminal_events_and_restart_mark_unfinished_tools_interrupted() -> Result<()> {
    for terminal in [None, Some(EventKind::Error), Some(EventKind::Status)] {
        let (directory, app, id) = application().await?;
        tool(
            &app,
            &id,
            "tool.execution_start",
            json!({"toolCallId":"a","toolName":"bash"}),
        );
        if let Some(kind) = terminal {
            receive(
                &app,
                &id,
                kind,
                json!({"state":"idle","message":"Disconnected"}),
            );
        }
        let reopened = App::open(AppOptions::local(directory.path().into()))?;
        let messages = reopened.store.messages(&id)?;
        let activity = messages[0].activity.as_ref().context("activity")?;
        assert_eq!(activity.status, crate::model::ActivityStatus::Interrupted);
        assert_eq!(
            activity.tools[0].status,
            crate::model::ActivityStatus::Interrupted
        );
    }
    Ok(())
}

#[tokio::test]
async fn empty_intents_and_internal_tools_never_hide_the_final_answer_or_approval() -> Result<()> {
    let (_directory, app, id) = application().await?;
    receive(
        &app,
        &id,
        EventKind::Intent,
        json!({"intent":"Almost done"}),
    );
    for name in ["report_intent", "builtin:task_complete"] {
        tool(
            &app,
            &id,
            "tool.execution_start",
            json!({"toolCallId":name,"toolName":name}),
        );
        tool(
            &app,
            &id,
            "tool.execution_complete",
            json!({"toolCallId":name,"success":true}),
        );
    }
    tool(
        &app,
        &id,
        "tool.execution_start",
        json!({"toolName":"view"}),
    );
    assert!(app.store.messages(&id)?.is_empty());
    receive(
        &app,
        &id,
        EventKind::Permission,
        json!({"requestId":"approval","kind":"shell","description":"Run tests"}),
    );
    assert_eq!(app.snapshot().await?.active_turns[&id].permissions.len(), 1);
    receive(
        &app,
        &id,
        EventKind::Message,
        json!({"id":"final","content":"Finished."}),
    );
    receive(&app, &id, EventKind::Status, json!({"state":"idle"}));
    let messages = app.store.messages(&id)?;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, MessageRole::Assistant);
    assert!(app.snapshot().await?.active_turns.is_empty());
    Ok(())
}

#[tokio::test]
async fn duplicate_message_ids_cannot_overwrite_users_or_other_conversations() -> Result<()> {
    let (_directory, app, id) = application().await?;
    let mut user: Message = serde_json::from_value(json!({
        "id":"user", "sessionId":id, "role":"user", "content":"Original", "createdAt":"same time",
        "attachments":[{"id":"attachment","name":"notes.txt","mimeType":"text/plain","size":8}]
    }))?;
    let contents = [b"Original".to_vec()];
    let saved = app
        .store
        .append_message_with_attachments(&user, &contents)?;
    assert!(saved.sequence.is_some());
    assert!(
        app.store
            .append_message_with_attachments(&user, &contents)
            .is_err()
    );
    user.role = MessageRole::Activity;
    user.activity = Some(crate::model::Activity {
        status: crate::model::ActivityStatus::Running,
        tools: Vec::new(),
    });
    assert!(matches!(
        app.store.save_activity(&user),
        Err(crate::store::StorageError::ActivityConflict { .. })
    ));

    receive(
        &app,
        &id,
        EventKind::Message,
        json!({"id":"assistant","content":"Progress"}),
    );
    let mut assistant = app.store.messages(&id)?[1].clone();
    assistant.role = MessageRole::Activity;
    assistant.activity = user.activity;
    let session = app.store.session(&id)?;
    let other = app
        .create_session(
            None,
            session.provider_id,
            session.model_id,
            Some(PermissionMode::Ask),
            None,
        )
        .await?;
    assistant.session_id = other.id.clone();
    assert!(matches!(
        app.store.save_activity(&assistant),
        Err(crate::store::StorageError::ActivityConflict { .. })
    ));
    assert!(app.store.messages(&other.id)?.is_empty());
    let messages = app.store.messages(&id)?;
    assert_eq!(messages[0].role, MessageRole::User);
    assert_eq!(messages[0].content, "Original");
    assert_eq!(messages[0].sequence, saved.sequence);
    assert_eq!(messages[0].attachments.len(), 1);
    assert_eq!(
        app.store.attachment(&id, "attachment")?,
        copilot_runtime::MessageAttachment::from_bytes("notes.txt", "text/plain", b"Original")
    );
    assert_eq!(messages[1].role, MessageRole::Assistant);
    Ok(())
}

#[tokio::test]
async fn plain_progress_is_absorbed_and_shell_output_does_not_duplicate_snapshots() -> Result<()> {
    let (_directory, app, id) = application().await?;
    receive(
        &app,
        &id,
        EventKind::Message,
        json!({"id":"progress","content":"Check the build","turnId":"1"}),
    );
    tool(
        &app,
        &id,
        "tool.execution_start",
        json!({"toolCallId":"shell","toolName":"bash","toolTitle":"Build","turnId":"1","arguments":{"command":"cargo check"}}),
    );
    tool(
        &app,
        &id,
        "tool.execution_partial_result",
        json!({"toolCallId":"shell","partialOutput":"hello"}),
    );
    tool(
        &app,
        &id,
        "tool.shell_output",
        json!({"toolCallId":"shell","sequence":0,"text":"hello"}),
    );
    tool(
        &app,
        &id,
        "tool.shell_output",
        json!({"toolCallId":"shell","sequence":1,"text":" world"}),
    );
    let live = app.store.messages(&id)?;
    assert_eq!(live.len(), 1);
    let activity = live[0].activity.as_ref().context("activity")?;
    assert_eq!(activity.tools[0].result, Some(json!("hello world")));
    assert_eq!(activity.tools[0].title.as_deref(), Some("Build"));
    tool(
        &app,
        &id,
        "tool.execution_complete",
        json!({"toolCallId":"shell","success":true,"result":{"content":"done","detailedContent":"complete build output"}}),
    );
    let history = app.store.messages(&id)?;
    assert_eq!(history[0].content, "Check the build");
    let tool = &history[0].activity.as_ref().context("activity")?.tools[0];
    assert_eq!(tool.arguments, Some(json!({"command":"cargo check"})));
    assert_eq!(
        tool.result.as_ref().context("result")?["detailedContent"],
        "complete build output"
    );
    Ok(())
}

async fn streaming_fleet_application() -> Result<(TempDir, Arc<App>, String)> {
    let (directory, app, id) = application().await?;
    for worker in ["worker-a", "worker-b"] {
        receive(
            &app,
            &id,
            EventKind::Agent,
            json!({
                "event":"subagent.started", "agentId":worker,
                "toolCallId":format!("dispatch-{worker}"), "agentName":"explore",
                "agentDisplayName":"Explore", "agentDescription":"Inspect a module",
                "model":"fixture"
            }),
        );
    }
    for (worker, content) in [
        (None, "Parent"),
        (Some("worker-a"), "Alpha"),
        (Some("worker-b"), "Beta"),
    ] {
        let mut delta = json!({"messageId":"same-sdk-id","content":content});
        let mut intent = json!({"intent":content});
        let mut started = json!({"event":"tool.execution_start", "details":{"toolCallId":"same-call-id","toolName":"view","arguments":{"path":content}}});
        if let Some(worker) = worker {
            delta["agentId"] = json!(worker);
            intent["agentId"] = json!(worker);
            started["agentId"] = json!(worker);
        }
        receive(&app, &id, EventKind::Delta, delta);
        receive(&app, &id, EventKind::Intent, intent);
        receive(&app, &id, EventKind::Tool, started);
    }
    Ok((directory, app, id))
}

#[tokio::test]
async fn fleet_outputs_and_tools_stay_scoped_across_live_snapshots_and_history() -> Result<()> {
    let (directory, app, id) = streaming_fleet_application().await?;
    let active = app.turns.snapshot()?;
    assert_eq!(active[&id].stream, "Parent");
    assert_eq!(active[&id].agent_streams["worker-a"], "Alpha");
    assert_eq!(active[&id].agent_streams["worker-b"], "Beta");
    receive(
        &app,
        &id,
        EventKind::Message,
        json!({"id":"same-sdk-id","content":"Alpha result","agentId":"worker-a"}),
    );
    assert!(
        !app.turns.snapshot()?[&id]
            .agent_streams
            .contains_key("worker-a")
    );
    assert_eq!(app.turns.snapshot()?[&id].stream, "Parent");
    assert_eq!(app.turns.snapshot()?[&id].agent_streams["worker-b"], "Beta");
    receive(
        &app,
        &id,
        EventKind::Agent,
        json!({"event":"subagent.completed","toolCallId":"dispatch-worker-a","agentId":"worker-a","agentName":"explore"}),
    );
    receive(&app, &id, EventKind::Status, json!({"state":"idle"}));
    assert!(app.turns.snapshot()?.is_empty());
    let saved = app.messages(&id)?;
    let agents: Vec<_> = saved
        .iter()
        .filter_map(|message| message.agent.as_ref())
        .collect();
    assert_eq!(agents.len(), 2);
    assert_eq!(
        agents
            .iter()
            .find(|agent| agent.id == "worker-a")
            .context("completed agent")?
            .status,
        crate::model::ActivityStatus::Completed
    );
    assert_eq!(
        agents
            .iter()
            .find(|agent| agent.id == "worker-b")
            .context("interrupted agent")?
            .status,
        crate::model::ActivityStatus::Interrupted
    );
    for worker in [None, Some("worker-a"), Some("worker-b")] {
        let activity = saved
            .iter()
            .find(|message| message.activity.is_some() && message.agent_id.as_deref() == worker)
            .context("scoped tool activity")?;
        assert_eq!(
            activity.activity.as_ref().context("activity")?.tools.len(),
            1
        );
    }
    let alpha = saved
        .iter()
        .find(|message| message.content == "Alpha result")
        .context("alpha reply")?;
    assert_eq!(alpha.agent_id.as_deref(), Some("worker-a"));
    assert!(!alpha.interrupted);
    let beta = saved
        .iter()
        .find(|message| message.content == "Beta" && message.role == MessageRole::Assistant)
        .context("partial beta reply")?;
    assert_eq!(beta.agent_id.as_deref(), Some("worker-b"));
    assert!(beta.interrupted);
    drop(app);
    let reopened = App::open(AppOptions::local(directory.path().into()))?;
    assert_eq!(reopened.messages(&id)?.len(), saved.len());
    Ok(())
}

#[tokio::test]
async fn unfinished_agents_are_interrupted_on_restart() -> Result<()> {
    let (directory, app, id) = application().await?;
    receive(
        &app,
        &id,
        EventKind::Agent,
        json!({"event":"subagent.started","toolCallId":"dispatch","agentId":"worker","agentName":"explore"}),
    );
    receive(
        &app,
        &id,
        EventKind::ReasoningDelta,
        json!({"agentId":"worker","reasoningId":"reasoning","content":"Inspecting"}),
    );
    let worker = app
        .messages(&id)?
        .into_iter()
        .find(|message| message.role == MessageRole::Reasoning)
        .context("worker reasoning")?;
    assert_eq!(worker.agent_id.as_deref(), Some("worker"));
    drop(app);
    let reopened = App::open(AppOptions::local(directory.path().into()))?;
    let saved = reopened.messages(&id)?;
    assert_eq!(
        saved
            .iter()
            .find_map(|message| message.agent.as_ref())
            .context("agent")?
            .status,
        crate::model::ActivityStatus::Interrupted
    );
    assert!(
        saved
            .iter()
            .filter(|message| message.role == MessageRole::Reasoning)
            .all(|message| message.interrupted)
    );
    assert!(reopened.store.title_exchange(&id)?.is_none());
    Ok(())
}

#[tokio::test]
async fn late_worker_completion_cannot_recreate_settled_cards_or_change_a_later_turn() -> Result<()>
{
    let (directory, app, id) = application().await?;
    receive(
        &app,
        &id,
        EventKind::Agent,
        json!({"event":"subagent.started","toolCallId":"stopped-call","agentId":"stopped-worker"}),
    );
    receive(&app, &id, EventKind::Status, json!({"state":"idle"}));
    let stopped = serde_json::to_value(app.messages(&id)?)?;
    assert_eq!(stopped[0]["agent"]["status"], "interrupted");
    for next_turn in [false, true] {
        if next_turn {
            receive(
                &app,
                &id,
                EventKind::Agent,
                json!({"event":"subagent.started","toolCallId":"next-call","agentId":"next-worker"}),
            );
        }
        let history = serde_json::to_value(app.messages(&id)?)?;
        let active = serde_json::to_value(app.turns.snapshot()?)?;
        for event in ["subagent.completed", "subagent.failed"] {
            receive(
                &app,
                &id,
                EventKind::Agent,
                json!({
                    "event":event,"toolCallId":"stopped-call","agentId":"stopped-worker",
                    "cancelled":true,"error":"Stopped worker"
                }),
            );
            assert_eq!(serde_json::to_value(app.messages(&id)?)?, history);
            assert_eq!(serde_json::to_value(app.turns.snapshot()?)?, active);
        }
    }
    receive(
        &app,
        &id,
        EventKind::Agent,
        json!({"event":"subagent.completed","toolCallId":"next-call","agentId":"next-worker"}),
    );
    let history = serde_json::to_value(app.messages(&id)?)?;
    assert_eq!(history.as_array().context("history")?.len(), 2);
    assert_eq!(history[0], stopped[0]);
    assert_eq!(history[1]["agent"]["status"], "completed");
    receive(&app, &id, EventKind::Status, json!({"state":"idle"}));
    let reopened = App::open(AppOptions::local(directory.path().into()))?;
    assert_eq!(serde_json::to_value(reopened.messages(&id)?)?, history);
    Ok(())
}

#[tokio::test]
async fn fleet_is_opt_in_and_rejected_for_chat_before_starting_the_runtime() -> Result<()> {
    let (_directory, app, id) = application().await?;
    let request: crate::Request = serde_json::from_value(
        json!({"command":"send_message","payload":{"sessionId":id,"content":"Research","messageId":Uuid::new_v4()}}),
    )?;
    assert!(matches!(
        request,
        crate::Request::SendMessage(crate::SendMessageRequest { fleet: false, .. })
    ));
    let error = app
        .send_message(&id, "Research".into(), Uuid::new_v4(), &[], &[], true)
        .await
        .err()
        .context("Chat must reject Fleet")?;
    assert!(matches!(error, Error::ChatFleetMode));
    assert!(app.messages(&id)?.is_empty());
    assert_eq!(app.store.session(&id)?.status, SessionStatus::Idle);
    Ok(())
}

#[tokio::test]
async fn worker_replies_never_replace_the_parent_reply_in_title_generation() -> Result<()> {
    let (_directory, app, id) = application().await?;
    let user: Message = serde_json::from_value(json!({
        "id":Uuid::new_v4(), "sessionId":id, "role":"user", "content":"Parent question",
        "attachments":[], "createdAt":now()
    }))?;
    app.store.append_message(&user)?;
    receive(
        &app,
        &id,
        EventKind::Message,
        json!({"id":"parent-reply", "content":"Parent answer"}),
    );
    receive(
        &app,
        &id,
        EventKind::Message,
        json!({"id":"worker-reply", "content":"Worker details", "agentId":"worker"}),
    );
    assert_eq!(
        app.store.title_exchange(&id)?,
        Some(("Parent question".into(), "Parent answer".into()))
    );
    Ok(())
}

#[tokio::test]
async fn a_worker_terminal_event_settles_only_its_output_and_late_text_reuses_the_checkpoint()
-> Result<()> {
    let (_directory, app, id) = streaming_fleet_application().await?;
    receive(
        &app,
        &id,
        EventKind::ReasoningDelta,
        json!({"reasoningId":"worker-thought","agentId":"worker-a","content":"Inspecting"}),
    );
    receive(
        &app,
        &id,
        EventKind::Agent,
        json!({
            "event":"subagent.failed", "toolCallId":"dispatch-worker-a", "agentId":"worker-a",
            "agentName":"explore", "error":"Worker stopped"
        }),
    );
    let active = app.turns.snapshot()?;
    assert_eq!(active[&id].stream, "Parent");
    assert_eq!(active[&id].agent_streams["worker-b"], "Beta");
    assert!(!active[&id].agent_streams.contains_key("worker-a"));
    let history = app.messages(&id)?;
    let checkpoint = history
        .iter()
        .find(|message| {
            message.role == MessageRole::Assistant
                && message.agent_id.as_deref() == Some("worker-a")
        })
        .context("partial worker output")?;
    assert_eq!(checkpoint.content, "Alpha");
    assert!(checkpoint.interrupted);
    for message in history
        .iter()
        .filter(|message| message.agent_id.as_deref() == Some("worker-a"))
    {
        assert!(
            message
                .activity
                .as_ref()
                .is_none_or(|activity| activity.status != crate::model::ActivityStatus::Running)
        );
        assert!(
            message
                .reasoning
                .as_ref()
                .is_none_or(|reasoning| reasoning.status != crate::model::ActivityStatus::Running)
        );
    }
    assert!(history.iter().any(|message| {
        message.agent_id.as_deref() == Some("worker-b")
            && message
                .activity
                .as_ref()
                .is_some_and(|activity| activity.status == crate::model::ActivityStatus::Running)
    }));
    receive(
        &app,
        &id,
        EventKind::Message,
        json!({"id":"same-sdk-id","agentId":"worker-a","content":"Final Alpha"}),
    );
    let updated = app.messages(&id)?;
    let final_output = updated
        .iter()
        .find(|message| message.id == checkpoint.id)
        .context("updated worker output")?;
    assert_eq!(final_output.content, "Final Alpha");
    assert!(!final_output.interrupted);
    assert_eq!(updated.len(), history.len());
    assert_eq!(app.turns.snapshot()?[&id].agent_streams["worker-b"], "Beta");
    Ok(())
}

#[tokio::test]
async fn usage_recording_failure_does_not_fail_the_conversation() -> Result<()> {
    let (directory, app, id) = application().await?;
    app.set_status(&id, SessionStatus::Running)?;
    let mut events = app.events.subscribe();
    let event = RuntimeEvent {
        session_id: id.clone(),
        kind: EventKind::Usage,
        data: json!({
            "eventId":"usage-1", "timestamp":"2026-10-22T00:00:00Z",
            "providerId":"fixture", "providerName":"Fixture", "model":"fixture",
            "inputTokens":100, "outputTokens":20,
        }),
    };
    assert!(matches!(app.receive_event(&event), EventOutcome::Continue));
    assert_eq!(events.try_recv()?.kind, "usage_updated");
    assert!(app.store.messages(&id)?.is_empty());
    let db = rusqlite::Connection::open(directory.path().join("tandem.sqlite3"))?;
    db.execute("DROP TABLE usage_events", [])?;
    assert!(matches!(app.receive_event(&event), EventOutcome::Continue));
    assert_eq!(events.try_recv()?.kind, "usage_error");
    assert_eq!(app.store.session(&id)?.status, SessionStatus::Running);
    Ok(())
}
