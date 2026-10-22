use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Weak},
};

use copilot_runtime::{Runtime, RuntimeOptions, TelemetrySettings};
use tandem_remote::ArtifactLocator;
use tokio::sync::{Mutex, broadcast};

use crate::{
    Error, Result,
    agent::LiveSession,
    model::{
        Event, FontSize, Preferences, RuntimeStatus, SessionStatus, Settings, Snapshot, Telemetry,
        ValidationError,
    },
    store::Store,
    submissions::Submissions,
    titles::TitleJobs,
    turns::ActiveTurns,
    vault::{LocalVault, SecretStore},
};

/// Installation paths and credential storage are injected at the application boundary.
pub struct AppOptions {
    pub data_directory: PathBuf,
    pub secrets: Arc<dyn SecretStore>,
    pub remote_artifacts: ArtifactLocator,
}

impl AppOptions {
    /// Use the chosen application directory for both settings and local credentials.
    /// Construction performs no I/O.
    #[must_use]
    pub fn local(data_directory: PathBuf) -> Self {
        Self {
            secrets: Arc::new(LocalVault::new(data_directory.join("tandem.sqlite3"))),
            data_directory,
            remote_artifacts: ArtifactLocator::default(),
        }
    }

    /// Resolve the per-user application directory.
    ///
    /// # Errors
    /// Returns an error if the platform does not expose a user data directory.
    pub fn native() -> Result<Self> {
        let directory = dirs::data_local_dir()
            .ok_or(Error::DataDirectoryUnavailable)?
            .join("app.tandem.desktop");
        Ok(Self::local(directory))
    }
}

/// Durable application state plus lazily started agent processes.
pub struct App {
    pub(crate) store: Store,
    pub(crate) options: AppOptions,
    pub(crate) events: broadcast::Sender<Event>,
    pub(crate) runtime: Mutex<Option<Arc<Runtime>>>,
    pub(crate) sessions: Mutex<HashMap<String, LiveSession>>,
    pub(crate) session_gates: Mutex<HashMap<String, Weak<Mutex<()>>>>,
    pub(crate) remote_environments: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub(crate) turns: ActiveTurns,
    pub(crate) submissions: Submissions,
    pub(crate) titles: TitleJobs,
    pub(crate) mutation_gate: Mutex<()>,
}

impl App {
    /// Open the local database. Starting the agent is intentionally deferred.
    ///
    /// # Errors
    /// Returns an error when the private data directory or database is inaccessible.
    pub fn open(options: AppOptions) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&options.data_directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                &options.data_directory,
                std::fs::Permissions::from_mode(0o700),
            )?;
        }
        let store = Store::open(&options.data_directory.join("tandem.sqlite3"))?;
        let (events, _) = broadcast::channel(4096);
        Ok(Arc::new(Self {
            store,
            options,
            events,
            runtime: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            session_gates: Mutex::new(HashMap::new()),
            remote_environments: Mutex::new(HashMap::new()),
            turns: ActiveTurns::default(),
            submissions: Submissions::default(),
            titles: TitleJobs::default(),
            mutation_gate: Mutex::new(()),
        }))
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub(crate) async fn snapshot(&self) -> Result<Snapshot> {
        let preferences = self.store.preferences()?;
        Ok(Snapshot {
            title_generations: self.titles.pending()?,
            preparing_sessions: self.submissions.preparing()?,
            settings: Settings {
                title_model: preferences.title_model,
                providers: self.store.providers()?,
                telemetry: preferences.telemetry,
                cli_path: preferences.cli_path,
                font_size: preferences.font_size,
            },
            projects: self.store.projects()?,
            sessions: self.store.sessions()?,
            runtime: self.runtime_status().await,
            active_turns: self.turns.snapshot()?,
        })
    }

    pub(crate) async fn runtime_status(&self) -> RuntimeStatus {
        let available = self.runtime.lock().await.is_some();
        let custom_cli = self
            .store
            .preferences()
            .ok()
            .and_then(|preferences| preferences.cli_path)
            .is_some_and(|path| !path.is_empty())
            || std::env::var_os("COPILOT_CLI_PATH").is_some_and(|path| !path.is_empty());
        RuntimeStatus {
            available,
            version: (!custom_cli).then(|| copilot_runtime::CLI_VERSION.into()),
            message: if available {
                "Copilot runtime is ready"
            } else if custom_cli {
                "Custom Copilot CLI configured; it will initialize on first use"
            } else {
                "Bundled Copilot runtime will initialize on first use"
            }
            .into(),
        }
    }

    pub(crate) async fn install_runtime(&self) -> Result<RuntimeStatus> {
        self.ensure_runtime().await?;
        Ok(self.runtime_status().await)
    }

    pub(crate) async fn ensure_runtime(&self) -> Result<Arc<Runtime>> {
        let mut runtime = self.runtime.lock().await;
        if let Some(runtime) = &*runtime
            && runtime.is_healthy().await
        {
            return Ok(runtime.clone());
        }
        if let Some(previous) = runtime.take()
            && let Err(error @ copilot_runtime::Error::Telemetry(_)) = previous.stop().await
        {
            return Err(error.into());
        }
        let preferences = self.store.preferences()?;
        let options = runtime_options(
            &preferences,
            self.options.data_directory.join("runtime"),
            self.telemetry_settings(&preferences.telemetry)?,
        );
        let running = Arc::new(
            Runtime::start(options)
                .await
                .map_err(Error::RuntimeStartup)?,
        );
        *runtime = Some(running.clone());
        Ok(running)
    }

    pub(crate) async fn save_font_size(&self, font_size: FontSize) -> Result<()> {
        let _guard = self.mutation_gate.lock().await;
        let mut preferences = self.store.preferences()?;
        preferences.font_size = font_size;
        Ok(self.store.save_preferences(&preferences)?)
    }

    pub(crate) async fn save_preferences(
        &self,
        telemetry: Telemetry,
        cli_path: Option<String>,
        telemetry_api_key: Option<String>,
    ) -> Result<()> {
        telemetry.validate()?;
        if let Some(path) = &cli_path
            && !path.is_empty()
            && !std::path::Path::new(path).is_file()
        {
            return Err(Error::CliNotFound { path: path.into() });
        }
        let _guard = self.mutation_gate.lock().await;
        self.require_idle()?;
        let telemetry_api_key = telemetry_api_key.map(|key| key.trim().to_owned());
        if telemetry.enabled && telemetry.agentlens.is_some() {
            let key = match &telemetry_api_key {
                Some(key) => Some(key.clone()),
                None => self.get_telemetry_api_key()?,
            };
            if key.as_deref().is_none_or(|key| key.trim().is_empty()) {
                return Err(ValidationError::MissingAgentLensApiKey.into());
            }
        }
        let _title_pause = self.titles.pause()?;
        self.update_title_configuration(|| {
            let mut preferences = self.store.preferences()?;
            preferences.telemetry = telemetry;
            preferences.cli_path = cli_path;
            let previous_key = telemetry_api_key
                .as_ref()
                .map(|_| self.get_telemetry_api_key())
                .transpose()?;
            if let Some(key) = telemetry_api_key {
                self.options.secrets.set_telemetry_api_key(Some(&key))?;
            }
            if let Err(storage) = self.store.save_preferences(&preferences) {
                if let Some(previous_key) = previous_key
                    && let Err(secret) = self
                        .options
                        .secrets
                        .set_telemetry_api_key(previous_key.as_deref())
                {
                    return Err(Error::TelemetryPreferencesRestore {
                        storage: Box::new(storage),
                        secret: Box::new(secret),
                    });
                }
                return Err(storage.into());
            }
            Ok(())
        })?;
        self.shutdown_sessions().await;
        if let Some(runtime) = self.runtime.lock().await.take() {
            runtime.stop().await?;
        }
        Ok(())
    }

    pub(crate) fn require_idle(&self) -> Result<()> {
        if self
            .store
            .sessions()?
            .iter()
            .any(|session| session.status == SessionStatus::Running)
        {
            return Err(Error::ConversationsRunning);
        }
        Ok(())
    }

    /// Stop sessions, flush telemetry, and reap agent processes before exit.
    ///
    /// # Errors
    /// Returns an error if the SDK process cannot be stopped cleanly.
    pub async fn shutdown(&self) -> Result<()> {
        self.titles.shutdown().await;
        self.shutdown_sessions().await;
        if let Some(runtime) = self.runtime.lock().await.take() {
            runtime.stop().await?;
        }
        Ok(())
    }
}

pub(crate) fn runtime_options(
    preferences: &Preferences,
    state_directory: PathBuf,
    telemetry: Option<TelemetrySettings>,
) -> RuntimeOptions {
    RuntimeOptions {
        state_directory,
        cli_path: preferences
            .cli_path
            .as_ref()
            .filter(|path| !path.is_empty())
            .map(PathBuf::from),
        telemetry,
    }
}

pub(crate) fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::{Request, model::PermissionMode};

    #[tokio::test]
    async fn font_size_defaults_to_medium_and_survives_preferences_saves_and_restart() -> Result<()>
    {
        let directory = TempDir::new()?;
        let app = App::open(AppOptions::local(directory.path().into()))?;
        assert_eq!(app.snapshot().await?.settings.font_size, FontSize::Medium);

        for (value, expected) in [
            ("small", FontSize::Small),
            ("medium", FontSize::Medium),
            ("large", FontSize::Large),
        ] {
            let request = serde_json::from_value(json!({
                "command": "save_font_size", "payload": {"fontSize": value}
            }))?;
            app.dispatch(request).await?;
            app.save_preferences(Telemetry::default(), None, None)
                .await?;
            assert_eq!(app.snapshot().await?.settings.font_size, expected);
        }

        for invalid in [json!("huge"), json!(""), json!(2), json!(null)] {
            assert!(
                serde_json::from_value::<Request>(json!({
                    "command": "save_font_size", "payload": {"fontSize": invalid}
                }))
                .is_err()
            );
        }
        assert!(
            serde_json::from_value::<Request>(json!({
                "command": "save_font_size", "payload": {}
            }))
            .is_err()
        );
        drop(app);
        let reopened = App::open(AppOptions::local(directory.path().into()))?;
        assert_eq!(
            reopened.snapshot().await?.settings.font_size,
            FontSize::Large
        );
        Ok(())
    }

    #[tokio::test]
    async fn changing_font_size_preserves_running_sessions_and_other_preferences() -> Result<()> {
        let directory = TempDir::new()?;
        let app = App::open(AppOptions::local(directory.path().into()))?;
        let provider = app.save_provider(serde_json::from_value(json!({
            "id": "", "name": "Fixture", "protocol": "anthropic", "baseUrl": "https://example.invalid",
            "models": [{"id": "chat", "name": "Chat", "contextWindow": 200_000}]
        }))?, None).await?;
        let mut session = app
            .create_session(
                None,
                provider.id,
                "chat".into(),
                Some(PermissionMode::Ask),
                None,
            )
            .await?;
        session.status = SessionStatus::Running;
        app.store.save_session(&session)?;
        let before = serde_json::to_value(app.store.preferences()?)?;

        app.dispatch(Request::SaveFontSize {
            font_size: FontSize::Large,
        })
        .await?;

        assert_eq!(
            app.store.session(&session.id)?.status,
            SessionStatus::Running
        );
        let after = serde_json::to_value(app.store.preferences()?)?;
        for key in ["telemetry", "cliPath", "titleModel"] {
            assert_eq!(after[key], before[key]);
        }
        assert_eq!(after["fontSize"], "large");
        Ok(())
    }
}
