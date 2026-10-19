use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::{
    App,
    app::now,
    model::{Session, SessionStatus},
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
        let session_guard = gate
            .try_lock_owned()
            .context("对话正在初始化或处理操作，请稍后切换模型")?;
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
            if !provider.models.iter().any(|model| model.id == model_id) {
                bail!("该供应商未配置选中的模型，请重新选择");
            }
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
        .context("模型已保存，但旧会话连接未能完成清理，请重试发送")?;
        Ok(updated)
    }

    fn editable_model_session(&self, id: &str) -> Result<Session> {
        let session = self.store.session(id)?;
        if session.status == SessionStatus::Running || self.turns.has_pending_permissions(id)? {
            bail!("请等待对话结束或停止对话后再切换模型");
        }
        Ok(session)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;
    use tempfile::TempDir;
    use uuid::Uuid;

    use super::*;
    use crate::{
        AppOptions, Request,
        model::{Connection, Message, MessageRole, PermissionMode, Project, Provider},
    };

    async fn application() -> Result<(TempDir, Arc<App>, Session, Provider)> {
        let directory = TempDir::new()?;
        let app = App::open(AppOptions::local(directory.path().into()))?;
        let original = app
            .save_provider(
                serde_json::from_value(json!({
                    "id":"", "name":"Original", "protocol":"openai-completions",
                    "baseUrl":"http://127.0.0.1:9999/v1",
                    "models":[{"id":"original","name":"Original"}]
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
            .create_session(None, original.id, "original".into(), PermissionMode::Ask)
            .await?;
        Ok((directory, app, session, alternate))
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
                    port: 22,
                    user: "fixture".into(),
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
                    created_at: now(),
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
            assert_eq!(model.context_window, Some(200_000));
            assert_eq!(model.max_output_tokens, Some(32_000));
            assert!(reopened.runtime.lock().await.is_none());
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
