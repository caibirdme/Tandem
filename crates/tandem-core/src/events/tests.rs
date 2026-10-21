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
            PermissionMode::Ask,
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
            PermissionMode::Ask,
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
