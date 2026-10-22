use crate::{
    App, Result,
    app::now,
    model::{Session, validate_reasoning_effort},
};

impl App {
    pub(crate) async fn set_session_reasoning_effort(
        &self,
        id: &str,
        reasoning_effort: Option<String>,
    ) -> Result<Session> {
        validate_reasoning_effort(reasoning_effort.as_deref())?;
        let _mutation_guard = self.mutation_gate.lock().await;
        let session = self.store.session(id)?;
        if session.reasoning_effort == reasoning_effort {
            return Ok(session);
        }
        // A running or prepared turn keeps its original settings. The next send
        // resolves this selection and resumes the SDK history if it changed.
        Ok(self.store.update_session(id, |session| {
            session.reasoning_effort = reasoning_effort;
            session.updated_at = now();
        })?)
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use anyhow::Result;
    use serde_json::json;
    use tempfile::TempDir;
    use uuid::Uuid;

    use super::*;
    use crate::{
        AppOptions, Error, ErrorCode, Request,
        model::{
            Connection, Message, MessageRole, PermissionMode, Project, SessionStatus,
            ValidationError,
        },
    };

    async fn application() -> Result<(TempDir, Arc<App>, Session)> {
        let directory = TempDir::new()?;
        let app = App::open(AppOptions::local(directory.path().into()))?;
        let provider = app.save_provider(
            serde_json::from_value(json!({
                "id":"", "name":"Fixture", "protocol":"openai-completions",
                "baseUrl":"http://127.0.0.1:9999/v1",
                "models":[{"id":"fixture", "name":"Fixture", "reasoningEffort":"medium", "contextWindow":200_000}]
            }))?,
            None,
        ).await?;
        let session = app
            .create_session(
                None,
                provider.id,
                "fixture".into(),
                Some(PermissionMode::Ask),
                None,
            )
            .await?;
        Ok((directory, app, session))
    }

    #[tokio::test]
    async fn reasoning_changes_preserve_chat_local_and_ssh_history_across_reopen() -> Result<()> {
        for connection in [
            None,
            Some(Connection::Local),
            Some(Connection::Ssh {
                host: "fixture.invalid".into(),
                port: Some(22),
                user: Some("fixture".into()),
                identity_file: None,
            }),
        ] {
            let (directory, app, mut session) = application().await?;
            if let Some(connection) = connection {
                let project = Project {
                    id: Uuid::new_v4().to_string(),
                    name: "Fixture work".into(),
                    path: "/fixture".into(),
                    connection,
                    created_at: now(),
                };
                app.store.save_project(&project)?;
                session.project_id = Some(project.id);
                session.permission_mode = PermissionMode::AllowAll;
                app.store.save_session(&session)?;
            }
            app.rename_session(&session.id, "Retained title").await?;
            for (role, content) in [
                (MessageRole::User, "Remember the request"),
                (MessageRole::Assistant, "Remember the answer"),
            ] {
                app.store.append_message(&Message {
                    id: Uuid::new_v4().to_string(),
                    session_id: session.id.clone(),
                    role,
                    content: content.into(),
                    attachments: Vec::new(),
                    created_at: now(),
                    sequence: None,
                    activity: None,
                    reasoning: None,
                    selected_skills: Vec::new(),
                    agent_id: None,
                    agent: None,
                    fleet: false,
                    interrupted: false,
                })?;
            }
            let history = serde_json::to_value(app.store.messages(&session.id)?)?;
            for effort in [Some("low"), Some("medium"), Some("high"), Some("max"), None] {
                let request: Request = serde_json::from_value(
                    json!({"command":"set_session_reasoning_effort", "payload":{"sessionId":session.id,"reasoningEffort":effort}}),
                )?;
                let updated: Session = serde_json::from_value(app.dispatch(request).await?)?;
                assert_eq!(updated.reasoning_effort.as_deref(), effort);
                assert_eq!(updated.id, session.id);
                assert_eq!(updated.project_id, session.project_id);
                assert_eq!(updated.provider_id, session.provider_id);
                assert_eq!(updated.model_id, session.model_id);
                assert_eq!(updated.permission_mode, session.permission_mode);
                assert_eq!(updated.title, "Retained title");
                assert_eq!(updated.created_at, session.created_at);
                let reopened = App::open(AppOptions::local(directory.path().into()))?;
                assert_eq!(
                    serde_json::to_value(reopened.store.session(&session.id)?)?,
                    serde_json::to_value(updated)?
                );
                assert_eq!(
                    serde_json::to_value(reopened.store.messages(&session.id)?)?,
                    history
                );
            }
            assert!(app.runtime.lock().await.is_none());
        }
        Ok(())
    }

    #[tokio::test]
    async fn updates_do_not_wait_for_initialization_or_interrupt_pending_work() -> Result<()> {
        for status in [
            SessionStatus::Idle,
            SessionStatus::Running,
            SessionStatus::Error,
        ] {
            let (_directory, app, session) = application().await?;
            app.set_status(&session.id, status)?;
            app.turns
                .permission(&session.id, &json!({"requestId":"pending"}))?;
            let before = serde_json::to_value(app.turns.snapshot()?)?;
            let gate = app.session_gate(&session.id).await;
            let _initialization = gate.lock().await;
            let updated = tokio::time::timeout(
                Duration::from_secs(1),
                app.set_session_reasoning_effort(&session.id, Some("high".into())),
            )
            .await??;
            assert_eq!(updated.reasoning_effort.as_deref(), Some("high"));
            assert_eq!(updated.status, status);
            assert_eq!(serde_json::to_value(app.turns.snapshot()?)?, before);
            // Events always update the latest stored session, preserving the selection.
            app.set_status(&session.id, SessionStatus::Idle)?;
            assert_eq!(
                app.store.session(&session.id)?.reasoning_effort.as_deref(),
                Some("high")
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn invalid_updates_are_typed_and_do_not_change_durable_settings() -> Result<()> {
        let (_directory, app, session) = application().await?;
        let before = serde_json::to_value(&session)?;
        for effort in ["", "LOW", " high", "xhigh", "unsupported"] {
            let error = app
                .set_session_reasoning_effort(&session.id, Some(effort.into()))
                .await
                .err()
                .ok_or_else(|| anyhow::anyhow!("invalid effort accepted"))?;
            assert_eq!(error.code(), ErrorCode::InvalidInput);
            assert!(matches!(
                error,
                Error::Validation(ValidationError::UnsupportedReasoningEffort { .. })
            ));
            assert_eq!(
                serde_json::to_value(app.store.session(&session.id)?)?,
                before
            );
            assert!(
                app.create_session(
                    None,
                    session.provider_id.clone(),
                    session.model_id.clone(),
                    Some(PermissionMode::Ask),
                    Some(effort.into())
                )
                .await
                .is_err()
            );
        }
        let error = app
            .set_session_reasoning_effort("missing", Some("high".into()))
            .await
            .err()
            .ok_or_else(|| anyhow::anyhow!("missing session accepted"))?;
        assert_eq!(error.code(), ErrorCode::NotFound);
        let unchanged = app.set_session_reasoning_effort(&session.id, None).await?;
        assert_eq!(serde_json::to_value(unchanged)?, before);
        Ok(())
    }

    #[test]
    fn updates_require_an_explicit_nullable_reasoning_effort() {
        for payload in [
            json!({"sessionId":"fixture"}),
            json!({"sessionId":"fixture","reasoningEffort":true}),
            json!({"sessionId":"fixture","reasoningEffort":7}),
        ] {
            assert!(
                serde_json::from_value::<Request>(
                    json!({"command":"set_session_reasoning_effort","payload":payload})
                )
                .is_err()
            );
        }
        for effort in [
            json!(null),
            json!("low"),
            json!("medium"),
            json!("high"),
            json!("max"),
        ] {
            assert!(serde_json::from_value::<Request>(json!({"command":"set_session_reasoning_effort","payload":{"sessionId":"fixture","reasoningEffort":effort}})).is_ok());
        }
    }
}
