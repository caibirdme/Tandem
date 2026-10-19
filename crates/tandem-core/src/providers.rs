use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::{
    App,
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
            Uuid::parse_str(&provider.id).context("供应商 ID 无效")?;
        }
        let _guard = self.mutation_gate.lock().await;
        self.require_provider_idle(&provider.id)?;
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
        for session in self
            .store
            .sessions()?
            .iter()
            .filter(|session| session.provider_id == provider.id)
        {
            self.disconnect_session(&session.id).await;
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
                self.store
                    .save_provider(&provider)
                    .with_context(|| format!("删除密钥失败（{error}），且无法恢复供应商记录"))?;
                return Err(error);
            }
            self.clear_invalid_title_model()
        })
    }

    pub(crate) fn get_provider_api_key(&self, id: &str) -> Result<Option<String>> {
        let provider = self.store.provider(id)?;
        if !provider.has_api_key {
            return Ok(None);
        }
        self.options.secrets.get(id)
    }

    fn require_provider_idle(&self, id: &str) -> Result<()> {
        if self
            .store
            .sessions()?
            .iter()
            .any(|session| session.provider_id == id && session.status == SessionStatus::Running)
        {
            bail!("请等待该供应商的对话结束后再修改配置");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use serde_json::json;
    use tempfile::TempDir;
    use tokio::sync::oneshot;

    use super::*;
    use crate::{AppOptions, MemoryVault, SecretStore};

    struct WaitingVault {
        keys: MemoryVault,
        entered: Mutex<Option<oneshot::Sender<()>>>,
        resume: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl WaitingVault {
        fn pause_once(&self) -> Result<()> {
            let entered = self
                .entered
                .lock()
                .map_err(|_| anyhow::anyhow!("fixture lock poisoned"))?
                .take();
            if let Some(entered) = entered {
                let _ = entered.send(());
                self.resume
                    .lock()
                    .map_err(|_| anyhow::anyhow!("fixture lock poisoned"))?
                    .recv_timeout(Duration::from_secs(5))?;
            }
            Ok(())
        }
    }

    impl SecretStore for WaitingVault {
        fn get(&self, id: &str) -> Result<Option<String>> {
            self.keys.get(id)
        }

        fn set(&self, id: &str, secret: Option<&str>) -> Result<()> {
            self.pause_once()?;
            self.keys.set(id, secret)
        }
    }

    fn provider() -> Result<Provider> {
        Ok(serde_json::from_value(json!({
            "id":Uuid::new_v4(), "name":"Fixture", "protocol":"anthropic",
            "baseUrl":"https://example.com", "hasApiKey":true,
            "models":[{"id":"fixture","name":"Fixture"}]
        }))?)
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
