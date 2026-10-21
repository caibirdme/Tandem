use uuid::Uuid;

use crate::{
    App, Error, Result,
    model::{Provider, SessionStatus},
};

impl App {
    pub(crate) async fn save_provider(
        &self,
        mut provider: Provider,
        api_key: Option<String>,
    ) -> Result<Provider> {
        provider.name = provider.name.trim().into();
        provider.base_url = provider.base_url.trim().trim_end_matches('/').into();
        provider.validate()?;
        if provider.id.is_empty() {
            provider.id = Uuid::new_v4().to_string();
        } else {
            Uuid::parse_str(&provider.id).map_err(Error::InvalidProviderId)?;
        }
        let _guard = self.mutation_gate.lock().await;
        let sessions = self
            .store
            .sessions()?
            .into_iter()
            .filter(|session| session.provider_id == provider.id)
            .collect::<Vec<_>>();
        if sessions
            .iter()
            .any(|session| session.status == SessionStatus::Running)
        {
            return Err(Error::ProviderRunning);
        }
        for session in &sessions {
            self.validate_attachment_history(&session.id, provider.protocol)?;
        }
        self.update_title_configuration(|| {
            let existing = self.store.optional_provider(&provider.id)?;
            if let Some(key) = api_key {
                let key = key.trim().to_owned();
                provider.has_api_key = !key.is_empty();
                self.options.secrets.set(&provider.id, Some(&key))?;
            } else if let Some(existing) = existing {
                provider.has_api_key =
                    existing.has_api_key && self.options.secrets.get(&provider.id)?.is_some();
            } else {
                self.options.secrets.set(&provider.id, None)?;
                provider.has_api_key = false;
            }
            self.store.save_provider(&provider)?;
            self.clear_invalid_title_model()
        })?;
        for session in sessions {
            self.disconnect_session(&session.id).await;
            if session.project_id.is_some() {
                self.store.update_session(&session.id, |session| {
                    session.context_usage = None;
                })?;
                self.emit(&session.id, "context_usage", serde_json::Value::Null);
            }
        }
        Ok(provider)
    }

    pub(crate) async fn delete_provider(&self, id: &str) -> Result<()> {
        let _guard = self.mutation_gate.lock().await;
        self.update_title_configuration(|| {
            let provider = self.store.provider(id)?;
            self.store.delete_provider(id)?;
            if let Err(error) = self.options.secrets.set(id, None) {
                // Keep the provider visible and retryable when local credential storage fails.
                if let Err(storage) = self.store.save_provider(&provider) {
                    return Err(Error::ProviderRestore {
                        secret: Box::new(error),
                        storage: Box::new(storage),
                    });
                }
                return Err(error.into());
            }
            self.clear_invalid_title_model()
        })
    }

    pub(crate) fn get_provider_api_key(&self, id: &str) -> Result<Option<String>> {
        let provider = self.store.provider(id)?;
        if !provider.has_api_key {
            return Ok(None);
        }
        Ok(self.options.secrets.get(id)?)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use anyhow::{Context, Result};
    use serde_json::json;
    use tempfile::TempDir;
    use tokio::sync::oneshot;

    use super::*;
    use crate::{
        AppOptions, MemoryVault, SecretError, SecretStore,
        app::now,
        model::{AttachmentMetadata, Message, MessageRole, PermissionMode, Protocol, Session},
    };

    struct WaitingVault {
        keys: MemoryVault,
        entered: Mutex<Option<oneshot::Sender<()>>>,
        resume: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl WaitingVault {
        fn pause_once(&self) {
            let entered = self
                .entered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(entered) = entered {
                let _ = entered.send(());
                assert!(
                    self.resume
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .recv_timeout(Duration::from_secs(5))
                        .is_ok()
                );
            }
        }
    }

    impl SecretStore for WaitingVault {
        fn get(&self, id: &str) -> std::result::Result<Option<String>, SecretError> {
            self.keys.get(id)
        }

        fn set(&self, id: &str, secret: Option<&str>) -> std::result::Result<(), SecretError> {
            self.pause_once();
            self.keys.set(id, secret)
        }
    }

    fn provider() -> Result<Provider> {
        Ok(serde_json::from_value(json!({
            "id":Uuid::new_v4(), "name":"Fixture", "protocol":"anthropic",
            "baseUrl":"https://example.com", "hasApiKey":true,
            "models":[{"id":"fixture","name":"Fixture","contextWindow":200_000}]
        }))?)
    }

    async fn conversation_with_attachment(
        app: &App,
        provider: &Provider,
        pdf: bool,
    ) -> Result<Session> {
        let session = app
            .create_session(
                None,
                provider.id.clone(),
                "fixture".into(),
                PermissionMode::Ask,
                None,
            )
            .await?;
        let bytes = if pdf {
            b"%PDF-1.7\n".as_slice()
        } else {
            b"text".as_slice()
        };
        app.store.append_message_with_attachments(
            &Message {
                id: Uuid::new_v4().to_string(),
                session_id: session.id.clone(),
                role: MessageRole::User,
                content: "Read this file".into(),
                attachments: vec![AttachmentMetadata {
                    id: Uuid::new_v4().to_string(),
                    name: if pdf { "report.pdf" } else { "notes.txt" }.into(),
                    mime_type: "application/octet-stream".into(),
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
            &[bytes.to_vec()],
        )?;
        Ok(session)
    }

    #[tokio::test]
    async fn provider_edits_preserve_attachment_compatibility_and_credentials() -> Result<()> {
        for (protocol, pdf, status, allowed) in [
            (
                Protocol::OpenaiCompletions,
                true,
                SessionStatus::Idle,
                false,
            ),
            (
                Protocol::OpenaiCompletions,
                true,
                SessionStatus::Error,
                false,
            ),
            (
                Protocol::OpenaiCompletions,
                true,
                SessionStatus::Running,
                false,
            ),
            (Protocol::OpenaiResponses, true, SessionStatus::Idle, true),
            (
                Protocol::OpenaiCompletions,
                false,
                SessionStatus::Idle,
                true,
            ),
        ] {
            let directory = TempDir::new()?;
            let app = App::open(AppOptions::local(directory.path().into()))?;
            let original = app
                .save_provider(provider()?, Some("original-key".into()))
                .await?;
            let session = conversation_with_attachment(&app, &original, pdf).await?;
            // Reopen before editing so saved conversations are checked without live agents.
            let app = App::open(AppOptions::local(directory.path().into()))?;
            app.set_status(&session.id, status)?;
            let history = serde_json::to_value(app.store.messages(&session.id)?)?;
            let mut edited = original.clone();
            edited.protocol = protocol;
            edited.name = "Edited provider".into();
            let result = app
                .save_provider(edited, Some("replacement-key".into()))
                .await;
            if allowed {
                let saved = result?;
                assert_eq!(saved.name, "Edited provider");
                assert_eq!(
                    app.get_provider_api_key(&original.id)?.as_deref(),
                    Some("replacement-key")
                );
            } else {
                let error = result
                    .err()
                    .context("incompatible provider edit must fail")?;
                if status == SessionStatus::Running {
                    assert!(matches!(error, Error::ProviderRunning));
                } else {
                    assert!(
                        matches!(&error, Error::ConversationAttachmentProtocol { session_id, source }
                        if session_id == &session.id
                        && matches!(source, copilot_runtime::AttachmentError::PdfUnsupportedByProvider))
                    );
                    assert_eq!(error.code(), crate::ErrorCode::InvalidInput);
                }
                assert_eq!(
                    serde_json::to_value(app.store.provider(&original.id)?)?,
                    serde_json::to_value(&original)?
                );
                assert_eq!(
                    app.get_provider_api_key(&original.id)?.as_deref(),
                    Some("original-key")
                );
            }
            assert_eq!(
                serde_json::to_value(app.store.messages(&session.id)?)?,
                history
            );
            assert_eq!(app.store.session(&session.id)?.status, status);
            assert!(app.runtime.lock().await.is_none());
        }
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_cannot_split_a_local_key_write_from_provider_metadata() -> Result<()> {
        let directory = TempDir::new()?;
        let (entered, started) = oneshot::channel();
        let (resume, waiting) = std::sync::mpsc::channel();
        let vault = Arc::new(WaitingVault {
            keys: MemoryVault::default(),
            entered: Mutex::new(Some(entered)),
            resume: Mutex::new(waiting),
        });
        let mut options = AppOptions::local(directory.path().into());
        options.secrets = vault.clone();
        let app = App::open(options)?;
        let provider = provider()?;
        let id = provider.id.clone();
        let saving = tokio::spawn({
            let app = Arc::clone(&app);
            async move {
                app.save_provider(provider, Some("fixture-local-key".into()))
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), started).await??;
        saving.abort();
        resume.send(())?;
        let _ = saving.await;
        assert!(app.store.provider(&id)?.has_api_key);
        assert_eq!(vault.get(&id)?.as_deref(), Some("fixture-local-key"));
        Ok(())
    }
}
