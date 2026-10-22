use std::{path::Path, process::Command};

use anyhow::{Context, Result};
use copilot_runtime::workspace::WorkspaceComparison;
use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::{AppOptions, Request, model::Message};

fn git(directory: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()?;
    anyhow::ensure!(output.status.success(), "Git fixture command failed");
    Ok(())
}

async fn fixture(directory: &TempDir) -> Result<(Arc<App>, Session, std::path::PathBuf)> {
    let project_path = directory.path().join("project");
    std::fs::create_dir(&project_path)?;
    git(&project_path, &["init", "-q"])?;
    std::fs::write(project_path.join("source.txt"), "original\n")?;
    git(&project_path, &["add", "--", "source.txt"])?;
    git(
        &project_path,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "Baseline",
        ],
    )?;
    let app = App::open(AppOptions::local(directory.path().join("data")))?;
    let provider: crate::model::Provider = serde_json::from_value(json!({
        "id":"","name":"Review test","protocol":"openai-completions",
        "baseUrl":"http://127.0.0.1:1/v1","models":[{"id":"test","name":"Test","contextWindow":32000}]
    }))?;
    let provider = app
        .save_provider(provider, Some("synthetic-key".into()))
        .await?;
    let project = app
        .create_project(
            "Review".into(),
            project_path.to_string_lossy().into(),
            crate::model::Connection::Local,
        )
        .await?;
    let session = app
        .create_session(Some(project.id), provider.id, "test".into(), None, None)
        .await?;
    Ok((app, session, project_path))
}

fn message(app: &App, session_id: &str, message_id: &str) -> Result<()> {
    let message: Message = serde_json::from_value(json!({
        "id":message_id,"sessionId":session_id,"role":"user","content":"Change source",
        "attachments":[],"createdAt":"2026-10-22T00:00:00Z"
    }))?;
    app.store.append_message(&message)?;
    Ok(())
}

async fn prepare_capture(
    app: &App,
    session: &Session,
    message_id: &str,
    available: bool,
) -> Result<()> {
    let handle = app.review_location(session, None).await?;
    let before = if available {
        Some(handle.capture(None).await?.id)
    } else {
        None
    };
    app.reviews.state.lock().await.active.insert(
        session.id.clone(),
        ActiveReview {
            handle: Some(handle),
            committed: false,
            record: TurnRecord {
                message_id: message_id.into(),
                captured_at: 1,
                before,
                after: None,
                complete: available,
                unavailable: None,
                summary: None,
            },
        },
    );
    Ok(())
}

#[tokio::test]
async fn workspace_review_is_read_only_and_revision_scoped_to_its_session() -> Result<()> {
    let directory = TempDir::new()?;
    let (app, session, project) = fixture(&directory).await?;
    std::fs::write(project.join("source.txt"), "updated\n")?;
    std::fs::write(project.join("new.txt"), "new\n")?;
    let overview = app
        .get_review_summary(
            &session.id,
            ReviewSelection::Workspace {
                comparison: WorkspaceComparison::Net,
            },
        )
        .await?;
    let summary = overview.summary.context("workspace summary")?;
    assert_eq!(summary.files_total, 2);
    assert!(app.runtime.lock().await.is_none());
    let files = app
        .list_review_files(&session.id, &summary.review_id, &summary.revision, 0)
        .await?;
    let file = app
        .get_review_file(
            &session.id,
            &summary.review_id,
            &summary.revision,
            &files.files[0].id,
        )
        .await?;
    let cached = serde_json::to_value(&file)?;
    let invalid = app
        .list_review_files(&session.id, &summary.review_id, "other-revision", 0)
        .await;
    assert!(matches!(invalid, Err(Error::ReviewRevisionExpired)));
    let other = app
        .create_session(
            session.project_id.clone(),
            session.provider_id.clone(),
            session.model_id.clone(),
            None,
            None,
        )
        .await?;
    let forbidden = app
        .get_review_file(
            &other.id,
            &summary.review_id,
            &summary.revision,
            &files.files[0].id,
        )
        .await;
    assert!(matches!(forbidden, Err(Error::ReviewNotRegistered)));
    std::fs::write(project.join("source.txt"), "later\n")?;
    std::fs::rename(&project, directory.path().join("moved-project"))?;
    drop(app);
    let restarted = App::open(AppOptions::local(directory.path().join("data")))?;
    let restored = restarted
        .get_review_file(
            &session.id,
            &summary.review_id,
            &summary.revision,
            &files.files[0].id,
        )
        .await?;
    assert_eq!(serde_json::to_value(restored)?, cached);
    Ok(())
}

#[tokio::test]
async fn oversized_cached_data_is_rejected_before_deserialization() -> Result<()> {
    let directory = TempDir::new()?;
    let path = directory.path().join("oversized.json");
    let file = std::fs::File::create(&path)?;
    file.set_len(2 * 1024 * 1024 + 1)?;
    let result = location::cached::<serde_json::Value>(&path).await;
    assert!(matches!(
        result,
        Err(Error::Review(
            copilot_runtime::workspace::ReviewError::Limit
        ))
    ));
    Ok(())
}

#[tokio::test]
async fn captured_turn_survives_commit_and_restart_without_rebasing_dirty_files() -> Result<()> {
    let directory = TempDir::new()?;
    let (app, session, project) = fixture(&directory).await?;
    std::fs::write(project.join("source.txt"), "user change\n")?;
    prepare_capture(&app, &session, "turn-one", true).await?;
    message(&app, &session.id, "turn-one")?;
    app.commit_review_turn(&session.id, "turn-one").await;
    std::fs::write(project.join("source.txt"), "user change\nagent change\n")?;
    git(&project, &["add", "--", "source.txt"])?;
    git(
        &project,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "Agent edit",
        ],
    )?;
    app.finish_review_turn(&session.id, true).await;
    let overview = app
        .get_review_summary(
            &session.id,
            ReviewSelection::Turn {
                message_id: "turn-one".into(),
            },
        )
        .await?;
    let summary = overview.summary.context("turn summary")?;
    assert_eq!(summary.additions, Some(1));
    assert_eq!(summary.deletions, Some(0));
    let workspace = app
        .get_review_summary(
            &session.id,
            ReviewSelection::Workspace {
                comparison: WorkspaceComparison::Net,
            },
        )
        .await?;
    assert_eq!(workspace.summary.context("clean workspace")?.files_total, 0);
    drop(app);
    let restarted = App::open(AppOptions::local(directory.path().join("data")))?;
    let restored = restarted
        .get_review_summary(&session.id, ReviewSelection::Session)
        .await?;
    assert_eq!(
        restored.summary.context("session summary")?.additions,
        Some(1)
    );
    Ok(())
}

#[tokio::test]
async fn failed_initial_baseline_is_not_replaced_by_a_later_turn() -> Result<()> {
    let directory = TempDir::new()?;
    let (app, session, project) = fixture(&directory).await?;
    prepare_capture(&app, &session, "cancelled", true).await?;
    app.abandon_review_turn(&session.id, "cancelled").await;
    assert!(
        app.store
            .review_session::<SessionRecord>(&session.id)?
            .is_none()
    );
    prepare_capture(&app, &session, "first", false).await?;
    message(&app, &session.id, "first")?;
    app.commit_review_turn(&session.id, "first").await;
    app.finish_review_turn(&session.id, true).await;
    prepare_capture(&app, &session, "second", true).await?;
    message(&app, &session.id, "second")?;
    app.commit_review_turn(&session.id, "second").await;
    std::fs::write(project.join("source.txt"), "changed\n")?;
    app.finish_review_turn(&session.id, true).await;
    let session_review = app
        .get_review_summary(&session.id, ReviewSelection::Session)
        .await?;
    assert!(session_review.summary.is_none());
    assert!(matches!(
        session_review.unavailable.map(|error| error.code),
        Some(UnavailableCode::BaselineUnavailable)
    ));
    let turn_review = app
        .get_review_summary(
            &session.id,
            ReviewSelection::Turn {
                message_id: "second".into(),
            },
        )
        .await?;
    assert_eq!(turn_review.summary.context("second turn")?.files_total, 1);
    Ok(())
}

#[tokio::test]
async fn bulk_turn_finishes_both_scopes_and_resolves_frozen_files_after_restart() -> Result<()> {
    let directory = TempDir::new()?;
    let (app, session, project) = fixture(&directory).await?;
    for index in 0..200 {
        std::fs::write(project.join(format!("bulk-{index:03}.txt")), "before\n")?;
    }
    prepare_capture(&app, &session, "bulk-turn", true).await?;
    message(&app, &session.id, "bulk-turn")?;
    app.commit_review_turn(&session.id, "bulk-turn").await;
    for index in 0..200 {
        std::fs::write(
            project.join(format!("bulk-{index:03}.txt")),
            "captured after\n",
        )?;
    }
    app.finish_review_turn(&session.id, true).await;
    let selections = [
        ReviewSelection::Session,
        ReviewSelection::Turn {
            message_id: "bulk-turn".into(),
        },
    ];
    for selection in selections {
        let overview = app.get_review_summary(&session.id, selection).await?;
        assert!(overview.unavailable.is_none());
        let summary = overview.summary.context("complete bulk review")?;
        assert!(summary.complete);
        assert_eq!(summary.files_total, 200);
        assert_eq!(summary.additions, None);
        assert_eq!(summary.deletions, None);
    }
    std::fs::write(project.join("bulk-000.txt"), "changed after snapshot\n")?;
    drop(app);
    let restarted = App::open(AppOptions::local(directory.path().join("data")))?;
    let summary = restarted
        .get_review_summary(&session.id, ReviewSelection::Session)
        .await?
        .summary
        .context("persisted bulk session")?;
    let page = restarted
        .list_review_files(&session.id, &summary.review_id, &summary.revision, 0)
        .await?;
    assert_eq!(page.files.len(), 200);
    assert!(page.next_cursor.is_none());
    let file = page.files.first().context("first bulk file")?;
    let detail = restarted
        .get_review_file(&session.id, &summary.review_id, &summary.revision, &file.id)
        .await?;
    assert_eq!(detail.file.additions, Some(1));
    assert_eq!(detail.file.deletions, Some(1));
    let patch = detail.patch.context("deferred bulk patch")?;
    assert!(patch.contains("+captured after"));
    assert!(!patch.contains("changed after snapshot"));
    Ok(())
}

#[tokio::test]
async fn non_repository_and_chat_have_distinct_review_outcomes() -> Result<()> {
    let directory = TempDir::new()?;
    let (app, session, _) = fixture(&directory).await?;
    let plain_path = directory.path().join("plain");
    std::fs::create_dir(&plain_path)?;
    let plain = app
        .create_project(
            "Plain".into(),
            plain_path.to_string_lossy().into(),
            crate::model::Connection::Local,
        )
        .await?;
    let plain_session = app
        .create_session(
            Some(plain.id),
            session.provider_id.clone(),
            session.model_id.clone(),
            None,
            None,
        )
        .await?;
    let overview = app
        .get_review_summary(
            &plain_session.id,
            ReviewSelection::Workspace {
                comparison: WorkspaceComparison::Net,
            },
        )
        .await?;
    assert!(matches!(
        overview.unavailable.map(|reason| reason.code),
        Some(UnavailableCode::NotRepository)
    ));
    let chat = app
        .create_session(None, session.provider_id, session.model_id, None, None)
        .await?;
    let error = app
        .dispatch(Request::GetReviewSummary(
            crate::dispatch::ReviewSummaryRequest {
                session_id: chat.id,
                selection: ReviewSelection::Session,
            },
        ))
        .await;
    assert!(matches!(error, Err(Error::WorkSessionRequired)));
    Ok(())
}

#[tokio::test]
async fn aborted_turn_stays_partial_and_startup_idle_preserves_preparation() -> Result<()> {
    let directory = TempDir::new()?;
    let (app, session, project) = fixture(&directory).await?;
    prepare_capture(&app, &session, "interrupted", true).await?;
    app.finish_review_turn(&session.id, true).await;
    assert!(
        app.reviews
            .state
            .lock()
            .await
            .active
            .contains_key(&session.id)
    );
    message(&app, &session.id, "interrupted")?;
    app.commit_review_turn(&session.id, "interrupted").await;
    std::fs::write(project.join("source.txt"), "partial change\n")?;
    app.interrupt_review_turn(&session.id).await;
    app.finish_review_turn(&session.id, true).await;
    let overview = app
        .get_review_summary(
            &session.id,
            ReviewSelection::Turn {
                message_id: "interrupted".into(),
            },
        )
        .await?;
    assert!(!overview.summary.context("partial turn summary")?.complete);
    assert!(!overview.turns[0].complete);
    Ok(())
}

#[tokio::test]
async fn unavailable_turn_retains_its_capture_failure() -> Result<()> {
    let directory = TempDir::new()?;
    let (app, session, _) = fixture(&directory).await?;
    app.reviews.state.lock().await.active.insert(
        session.id.clone(),
        ActiveReview {
            handle: None,
            committed: false,
            record: TurnRecord {
                message_id: "offline".into(),
                captured_at: 1,
                before: None,
                after: None,
                complete: false,
                summary: None,
                unavailable: Some(ReviewUnavailable {
                    code: UnavailableCode::RemoteUnavailable,
                    message: "The remote host is unavailable.".into(),
                }),
            },
        },
    );
    message(&app, &session.id, "offline")?;
    app.commit_review_turn(&session.id, "offline").await;
    app.finish_review_turn(&session.id, false).await;
    let overview = app
        .get_review_summary(
            &session.id,
            ReviewSelection::Turn {
                message_id: "offline".into(),
            },
        )
        .await?;
    assert!(overview.summary.is_none());
    assert!(!overview.turns[0].available);
    assert!(matches!(
        overview.unavailable.map(|reason| reason.code),
        Some(UnavailableCode::RemoteUnavailable)
    ));
    Ok(())
}

#[tokio::test]
async fn failed_new_endpoint_never_returns_the_previous_session_summary() -> Result<()> {
    let directory = TempDir::new()?;
    let (app, session, project) = fixture(&directory).await?;
    prepare_capture(&app, &session, "first", true).await?;
    message(&app, &session.id, "first")?;
    app.commit_review_turn(&session.id, "first").await;
    std::fs::write(project.join("source.txt"), "first change\n")?;
    app.finish_review_turn(&session.id, true).await;
    assert!(
        app.get_review_summary(&session.id, ReviewSelection::Session)
            .await?
            .summary
            .is_some()
    );
    prepare_capture(&app, &session, "second", true).await?;
    message(&app, &session.id, "second")?;
    app.commit_review_turn(&session.id, "second").await;
    let pending = app
        .get_review_summary(&session.id, ReviewSelection::Session)
        .await?;
    assert!(pending.summary.is_none());
    assert!(matches!(
        pending.unavailable.map(|reason| reason.code),
        Some(UnavailableCode::Capturing)
    ));
    std::fs::rename(&project, directory.path().join("moved"))?;
    app.finish_review_turn(&session.id, true).await;
    let failed = app
        .get_review_summary(&session.id, ReviewSelection::Session)
        .await?;
    assert!(failed.summary.is_none());
    assert!(matches!(
        failed.unavailable.map(|reason| reason.code),
        Some(UnavailableCode::InspectionFailed)
    ));
    let previous = app
        .get_review_summary(
            &session.id,
            ReviewSelection::Turn {
                message_id: "first".into(),
            },
        )
        .await?;
    assert!(previous.summary.is_some());
    Ok(())
}
