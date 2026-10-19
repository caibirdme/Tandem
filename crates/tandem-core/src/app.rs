use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Weak},
};

use copilot_runtime::{Runtime, RuntimeOptions, TelemetryProtocol, TelemetrySettings};
use tandem_remote::ArtifactLocator;
use tokio::sync::{Mutex, broadcast};

use crate::{
    Error, Result,
    agent::LiveSession,
    model::{Event, Preferences, RuntimeStatus, SessionStatus, Settings, Snapshot, Telemetry},
    store::Store,
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
    pub(crate) turns: ActiveTurns,
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
            turns: ActiveTurns::default(),
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
            settings: Settings {
                title_model: preferences.title_model,
                providers: self.store.providers()?,
                telemetry: preferences.telemetry,
                cli_path: preferences.cli_path,
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

    pub(crate) async fn ensure_runtime(&self) -> Result<Arc<Runtime>> {
        let mut runtime = self.runtime.lock().await;
        if let Some(runtime) = &*runtime
            && runtime.is_healthy().await
        {
            return Ok(runtime.clone());
        }
        if let Some(previous) = runtime.take() {
            let _ = previous.stop().await;
        }
        let preferences = self.store.preferences()?;
        let options = runtime_options(&preferences, self.options.data_directory.join("runtime"));
        let running = Arc::new(
            Runtime::start(options)
                .await
                .map_err(Error::RuntimeStartup)?,
        );
        *runtime = Some(running.clone());
        Ok(running)
    }

    pub(crate) async fn save_preferences(
        &self,
        telemetry: Telemetry,
        cli_path: Option<String>,
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
        let _title_pause = self.titles.pause()?;
        self.update_title_configuration(|| {
            let mut preferences = self.store.preferences()?;
            preferences.telemetry = telemetry;
            preferences.cli_path = cli_path;
            Ok(self.store.save_preferences(&preferences)?)
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
) -> RuntimeOptions {
    let telemetry = preferences.telemetry.enabled.then(|| TelemetrySettings {
        endpoint: preferences.telemetry.endpoint.clone(),
        protocol: if preferences.telemetry.protocol == "http/json" {
            TelemetryProtocol::HttpJson
        } else {
            TelemetryProtocol::HttpProtobuf
        },
        capture_content: preferences.telemetry.capture_content,
    });
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
