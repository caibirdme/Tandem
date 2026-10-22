use std::sync::Arc;

use copilot_runtime::validate_attachment_protocol;

use crate::{
    App, Error, Result,
    app::now,
    model::{Protocol, Session, SessionStatus},
};

impl App {
    pub(crate) async fn set_session_model(
        self: &Arc<Self>,
        id: &str,
        provider_id: String,
        model_id: String,
    ) -> Result<Session> {
        self.editable_model_session(id)?;
        let gate = self.session_gate(id).await;
        let session_guard = gate.try_lock_owned().map_err(|_| Error::ModelChangeBusy)?;
        let previous = self
            .sessions
            .lock()
            .await
            .get(id)
            .map(|live| live.completion.clone());
        if let Some(completion) = previous {
            completion.wait().await?;
        }
        let updated = {
            let _mutation_guard = self.mutation_gate.lock().await;
            let session = self.editable_model_session(id)?;
            let provider = self.store.provider(&provider_id)?;
            let model = provider
                .models
                .iter()
                .find(|model| model.id == model_id)
                .ok_or_else(|| Error::ModelNotConfigured {
                    provider_id: provider_id.clone(),
                    model_id: model_id.clone(),
                })?;
            let reset_effort =
                !model.supports_reasoning_effort(session.reasoning_effort.as_deref());
            self.validate_attachment_history(id, provider.protocol)?;
            if session.provider_id == provider_id && session.model_id == model_id {
                return Ok(session);
            }
            let sessions = self.sessions.lock().await;
            // Invalidation and the durable selection have no cancellation point.
            // A cancelled request can therefore never reuse the old configuration.
            if let Some(live) = sessions.get(id) {
                live.completion.fail();
            }
            self.store.update_session(id, |session| {
                session.provider_id = provider_id;
                session.model_id = model_id;
                if reset_effort {
                    session.reasoning_effort = None;
                }
                session.context_usage = None;
                session.updated_at = now();
            })?
        };
        let app = Arc::clone(self);
        let session_id = id.to_owned();
        // Keep this gate until cleanup finishes even if the caller is cancelled.
        // Resuming the next turn must happen after the old connection is detached.
        tokio::spawn(async move {
            let _session_guard = session_guard;
            app.disconnect_session(&session_id).await;
        })
        .await
        .map_err(Error::ModelConnectionCleanup)?;
        Ok(updated)
    }

    pub(crate) fn validate_attachment_history(&self, id: &str, protocol: Protocol) -> Result<()> {
        for (name, mime_type) in self.store.attachment_types(id)? {
            validate_attachment_protocol(&name, &mime_type, protocol.into()).map_err(|source| {
                Error::ConversationAttachmentProtocol {
                    session_id: id.into(),
                    source,
                }
            })?;
        }
        Ok(())
    }

    fn editable_model_session(&self, id: &str) -> Result<Session> {
        let session = self.store.session(id)?;
        if session.status == SessionStatus::Running || self.turns.has_pending_permissions(id)? {
            return Err(Error::ModelChangeRunning);
        }
        Ok(session)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use anyhow::{Context, Result};
    use serde_json::json;
    use tempfile::TempDir;
    use uuid::Uuid;

    use super::*;
    use crate::{
        AppOptions, Request,
        model::{
            AttachmentMetadata, Connection, Message, MessageRole, PermissionMode, Project, Provider,
        },
    };

    async fn application() -> Result<(TempDir, Arc<App>, Session, Provider)> {
        let directory = TempDir::new()?;
        let app = App::open(AppOptions::local(directory.path().into()))?;
        let original = app
            .save_provider(
                serde_json::from_value(json!({
                    "id":"", "name":"Original", "protocol":"openai-completions",
                    "baseUrl":"http://127.0.0.1:9999/v1",
                    "models":[{"id":"original","name":"Original","contextWindow":200_000}]
                }))?,
                None,
            )
            .await?;
        let alternate = app
            .save_provider(
                serde_json::from_value(json!({
                    "id":"", "name":"Alternate", "protocol":"anthropic",
                    "baseUrl":"http://127.0.0.1:9998",
                    "models":[{"id":"alternate","name":"Alternate","reasoningEffort":"high",
                        "contextWindow":200_000,"maxOutputTokens":32_000}]
                }))?,
                None,
            )
            .await?;
        let session = app
            .create_session(
                None,
                original.id,
                "original".into(),
                Some(PermissionMode::Ask),
                None,
            )
            .await?;
        Ok((directory, app, session, alternate))
    }

    #[tokio::test]
    async fn pdf_history_prevents_unsupported_model_switches_without_changing_state() -> Result<()>
    {
        for (mime_type, connection) in [
            ("application/pdf", None),
            ("application/octet-stream", Some(Connection::Local)),
            (
                "application/pdf",
                Some(Connection::Ssh {
                    host: "fixture.invalid".into(),
                    port: Some(22),
                    user: Some("fixture".into()),
                    identity_file: None,
                }),
            ),
        ] {
            let (directory, app, mut session, alternate) = application().await?;
            let completions_provider = session.provider_id.clone();
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
                app.store.save_session(&session)?;
            }
            session = app
                .set_session_model(&session.id, alternate.id.clone(), "alternate".into())
                .await?;
            let bytes = b"%PDF-1.7\n".to_vec();
            let attachment_id = Uuid::new_v4().to_string();
            app.store.append_message_with_attachments(
                &Message {
                    id: Uuid::new_v4().to_string(),
                    session_id: session.id.clone(),
                    role: MessageRole::User,
                    content: "Read this PDF".into(),
                    attachments: vec![AttachmentMetadata {
                        id: attachment_id.clone(),
                        name: "REPORT.PDF".into(),
                        mime_type: mime_type.into(),
                        size: bytes.len(),
                    }],
                    created_at: now(),
                    sequence: None,
                    activity: None,
                    reasoning: None,
                    selected_skills: Vec::new(),
                    agent_id: None,
                    agent: None,
                    fleet: false,
                    interrupted: false,
                },
                &[bytes],
            )?;
            let before = serde_json::to_value(&session)?;
            let history = serde_json::to_value(app.store.messages(&session.id)?)?;
            let error = app
                .set_session_model(&session.id, completions_provider, "original".into())
                .await
                .err()
                .context("PDF history must block an incompatible model switch")?;
            assert!(
                matches!(&error, Error::ConversationAttachmentProtocol { session_id, source }
                if session_id == &session.id
                && matches!(source, copilot_runtime::AttachmentError::PdfUnsupportedByProvider))
            );
            assert_eq!(error.code(), crate::ErrorCode::InvalidInput);
            assert_eq!(
                serde_json::to_value(app.store.session(&session.id)?)?,
                before
            );
            let mut responses = alternate;
            responses.id.clear();
            responses.protocol = Protocol::OpenaiResponses;
            let responses = app.save_provider(responses, None).await?;
            let switched = app
                .set_session_model(&session.id, responses.id.clone(), "alternate".into())
                .await?;
            assert_eq!(switched.provider_id, responses.id);
            let reopened = App::open(AppOptions::local(directory.path().into()))?;
            assert_eq!(
                serde_json::to_value(reopened.store.messages(&session.id)?)?,
                history
            );
            assert!(
                reopened
                    .store
                    .attachment(&session.id, &attachment_id)
                    .is_ok()
            );
            assert!(app.runtime.lock().await.is_none());
        }
        Ok(())
    }

    #[tokio::test]
    async fn model_changes_preserve_chat_and_work_history_across_reopen() -> Result<()> {
        for (connection, mode, status) in [
            (None, PermissionMode::Ask, SessionStatus::Idle),
            (
                Some(Connection::Local),
                PermissionMode::AllowAll,
                SessionStatus::Error,
            ),
            (
                Some(Connection::Ssh {
                    host: "fixture.invalid".into(),
                    port: Some(22),
                    user: Some("fixture".into()),
                    identity_file: None,
                }),
                PermissionMode::Ask,
                SessionStatus::Idle,
            ),
        ] {
            let (directory, app, mut session, alternate) = application().await?;
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
                session.permission_mode = mode;
                session.context_usage = Some(copilot_runtime::ContextUsage {
                    current_tokens: 153_000,
                    token_limit: 200_000,
                });
                app.store.save_session(&session)?;
            }
            app.rename_session(&session.id, "Keep this title").await?;
            app.set_status(&session.id, status)?;
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
            let request: Request = serde_json::from_value(json!({
                "command":"set_session_model",
                "payload":{"sessionId":session.id,"providerId":alternate.id,"modelId":"alternate"},
            }))?;
            let updated: Session = serde_json::from_value(app.dispatch(request).await?)?;
            assert_eq!(updated.id, session.id);
            assert_eq!(updated.provider_id, alternate.id);
            assert_eq!(updated.model_id, "alternate");
            assert_eq!(updated.project_id, session.project_id);
            assert_eq!(updated.permission_mode, mode);
            assert_eq!(updated.title, "Keep this title");
            assert_eq!(updated.status, status);
            assert_eq!(updated.created_at, session.created_at);
            assert!(updated.context_usage.is_none());
            let reopened = App::open(AppOptions::local(directory.path().into()))?;
            let stored = reopened.store.session(&session.id)?;
            assert_eq!(
                serde_json::to_value(stored.clone())?,
                serde_json::to_value(updated)?
            );
            assert_eq!(
                serde_json::to_value(reopened.store.messages(&session.id)?)?,
                history
            );
            let (provider, model) =
                reopened.model_settings(&stored.provider_id, &stored.model_id)?;
            assert_eq!(provider.base_url, alternate.base_url);
            assert_eq!(model.id, "alternate");
            assert_eq!(model.reasoning_effort.as_deref(), Some("high"));
            assert_eq!(model.context_window, 200_000);
            assert_eq!(model.max_output_tokens, Some(32_000));
            assert!(reopened.runtime.lock().await.is_none());
        }
        Ok(())
    }

    #[tokio::test]
    async fn model_switch_resets_only_overrides_outside_reported_capabilities() -> Result<()> {
        for supported in [None, Some(vec![]), Some(vec!["low", "xhigh"])] {
            for effort in [None, Some("low"), Some("xhigh"), Some("max")] {
                let (directory, app, session, mut alternate) = application().await?;
                alternate.models[0].reasoning_effort = None;
                alternate.models[0].supported_reasoning_efforts = supported
                    .as_ref()
                    .map(|values| values.iter().map(|value| (*value).into()).collect());
                let alternate = app.save_provider(alternate, None).await?;
                app.set_session_reasoning_effort(&session.id, effort.map(str::to_owned))
                    .await?;
                let expected = effort.filter(|effort| {
                    supported
                        .as_ref()
                        .is_none_or(|values| values.contains(effort))
                });
                let updated = app
                    .set_session_model(&session.id, alternate.id.clone(), "alternate".into())
                    .await?;
                assert_eq!(updated.reasoning_effort.as_deref(), expected);
                assert_eq!(updated.id, session.id);
                assert_eq!(updated.provider_id, alternate.id);
                assert_eq!(updated.model_id, "alternate");
                drop(app);
                let app = App::open(AppOptions::local(directory.path().into()))?;
                assert_eq!(
                    app.store.session(&session.id)?.reasoning_effort.as_deref(),
                    expected
                );
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn invalid_model_selections_leave_the_session_unchanged() -> Result<()> {
        let (_directory, app, session, alternate) = application().await?;
        let before = serde_json::to_value(&session)?;
        for (id, provider_id, model_id) in [
            ("missing", alternate.id.as_str(), "alternate"),
            (session.id.as_str(), "missing", "alternate"),
            (session.id.as_str(), alternate.id.as_str(), "original"),
            (session.id.as_str(), alternate.id.as_str(), ""),
        ] {
            assert!(
                app.set_session_model(id, provider_id.into(), model_id.into())
                    .await
                    .is_err()
            );
            assert_eq!(
                serde_json::to_value(app.store.session(&session.id)?)?,
                before
            );
        }
        let unchanged = app
            .set_session_model(
                &session.id,
                session.provider_id.clone(),
                session.model_id.clone(),
            )
            .await?;
        assert_eq!(serde_json::to_value(unchanged)?, before);
        Ok(())
    }

    #[tokio::test]
    async fn model_changes_reject_running_initializing_and_pending_sessions_without_waiting()
    -> Result<()> {
        for (status, pending, initializing) in [
            (SessionStatus::Running, false, false),
            (SessionStatus::Idle, true, false),
            (SessionStatus::Error, true, false),
            (SessionStatus::Idle, false, true),
            (SessionStatus::Error, false, true),
        ] {
            let (_directory, app, session, alternate) = application().await?;
            app.set_status(&session.id, status)?;
            if pending {
                app.turns
                    .permission(&session.id, &json!({"requestId":"pending"}))?;
            }
            let gate = app.session_gate(&session.id).await;
            let _initialization = initializing.then(|| gate.try_lock()).transpose()?;
            let _configuration = app.mutation_gate.lock().await;
            let result = tokio::time::timeout(
                Duration::from_secs(1),
                app.set_session_model(&session.id, alternate.id, "alternate".into()),
            )
            .await?;
            assert!(result.is_err(), "{status:?}/{pending}/{initializing}");
            let stored = app.store.session(&session.id)?;
            assert_eq!(stored.provider_id, session.provider_id);
            assert_eq!(stored.model_id, session.model_id);
            assert_eq!(stored.status, status);
        }
        Ok(())
    }

    #[tokio::test]
    async fn provider_foreign_keys_follow_both_session_write_paths() -> Result<()> {
        for save_whole_session in [false, true] {
            let (_directory, app, mut session, alternate) = application().await?;
            let original_id = session.provider_id.clone();
            if save_whole_session {
                session.provider_id = alternate.id.clone();
                session.model_id = "alternate".into();
                app.store.save_session(&session)?;
            } else {
                app.set_session_model(&session.id, alternate.id.clone(), "alternate".into())
                    .await?;
            }
            app.delete_provider(&original_id).await?;
            assert!(app.delete_provider(&alternate.id).await.is_err());
            assert_eq!(app.store.session(&session.id)?.provider_id, alternate.id);
            app.delete_session(&session.id).await?;
            app.delete_provider(&alternate.id).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn model_changes_revalidate_after_waiting_for_configuration_updates() -> Result<()> {
        let (_directory, app, session, mut alternate) = application().await?;
        let configuration = app.mutation_gate.lock().await;
        let changing = tokio::spawn({
            let app = Arc::clone(&app);
            let id = session.id.clone();
            let provider_id = alternate.id.clone();
            async move {
                app.set_session_model(&id, provider_id, "alternate".into())
                    .await
            }
        });
        let gate = app.session_gate(&session.id).await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while gate.try_lock().is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert!(
            app.set_session_model(&session.id, session.provider_id.clone(), "original".into())
                .await
                .is_err()
        );
        alternate.models[0].id = "replacement".into();
        app.store.save_provider(&alternate)?;
        drop(configuration);
        assert!(changing.await?.is_err());
        assert_eq!(
            app.store.session(&session.id)?.provider_id,
            session.provider_id
        );
        assert!(gate.try_lock().is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn same_provider_model_changes_keep_a_concurrently_completed_title() -> Result<()> {
        let (_directory, app, session, _alternate) = application().await?;
        let mut provider = app.store.provider(&session.provider_id)?;
        let mut model = provider.models[0].clone();
        model.id = "second".into();
        provider.models.push(model);
        app.save_provider(provider, None).await?;
        let configuration = app.mutation_gate.lock().await;
        let changing = tokio::spawn({
            let app = Arc::clone(&app);
            let id = session.id.clone();
            let provider_id = session.provider_id.clone();
            async move {
                app.set_session_model(&id, provider_id, "second".into())
                    .await
            }
        });
        let gate = app.session_gate(&session.id).await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while gate.try_lock().is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        // Title completion intentionally does not acquire the async mutation gate.
        app.store.update_session(&session.id, |session| {
            session.title = "Concurrent generated title".into();
        })?;
        drop(configuration);
        let updated = changing.await??;
        assert_eq!(updated.provider_id, session.provider_id);
        assert_eq!(updated.model_id, "second");
        assert_eq!(updated.title, "Concurrent generated title");
        assert_eq!(app.store.session(&session.id)?.title, updated.title);
        Ok(())
    }

    #[tokio::test]
    async fn cancelling_a_waiting_model_change_leaves_its_selection_and_gate_usable() -> Result<()>
    {
        let (_directory, app, session, alternate) = application().await?;
        let configuration = app.mutation_gate.lock().await;
        let changing = tokio::spawn({
            let app = Arc::clone(&app);
            let id = session.id.clone();
            async move {
                app.set_session_model(&id, alternate.id, "alternate".into())
                    .await
            }
        });
        let gate = app.session_gate(&session.id).await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while gate.try_lock().is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        changing.abort();
        let _ = changing.await;
        assert!(gate.try_lock().is_ok());
        assert_eq!(
            serde_json::to_value(app.store.session(&session.id)?)?,
            serde_json::to_value(session)?
        );
        drop(configuration);
        Ok(())
    }
}
