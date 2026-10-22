use std::{collections::HashMap, path::PathBuf, sync::Mutex, time::Duration};

use rusqlite::{Connection, OptionalExtension, params};

/// Credential persistence failures, without including credential values.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("Cannot open the local API key database: {source}")]
    OpenDatabase {
        #[source]
        source: rusqlite::Error,
    },
    #[error("Cannot access the local API key database: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("Credential lock is unavailable. Restart the application.")]
    LockPoisoned,
}

type Result<T> = std::result::Result<T, SecretError>;

/// Credential persistence is separate from provider metadata and frontend snapshots.
pub trait SecretStore: Send + Sync {
    /// Read a credential, returning `None` when it has not been configured.
    ///
    /// # Errors
    /// Returns an error when credential storage is unavailable.
    fn get(&self, id: &str) -> Result<Option<String>>;
    /// Replace or remove a credential.
    ///
    /// # Errors
    /// Returns an error when credential storage is unavailable.
    fn set(&self, id: &str, secret: Option<&str>) -> Result<()>;
    /// Read the `AgentLens` credential on demand.
    ///
    /// # Errors
    /// Returns an error when credential storage is unavailable.
    fn get_telemetry_api_key(&self) -> Result<Option<String>>;
    /// Replace or remove the `AgentLens` credential independently of provider keys.
    ///
    /// # Errors
    /// Returns an error when credential storage is unavailable.
    fn set_telemetry_api_key(&self, secret: Option<&str>) -> Result<()>;
}

/// Plaintext credentials stored separately from provider metadata in the local database.
pub struct LocalVault {
    database: PathBuf,
}

impl LocalVault {
    /// Construct a lazy store. The database's parent directory must exist before use.
    #[must_use]
    pub fn new(database: PathBuf) -> Self {
        Self { database }
    }

    fn open(&self) -> Result<Connection> {
        let database = Connection::open(&self.database)
            .map_err(|source| SecretError::OpenDatabase { source })?;
        database.busy_timeout(Duration::from_secs(5))?;
        database.execute_batch(
            "CREATE TABLE IF NOT EXISTS provider_secrets (
                id TEXT PRIMARY KEY, secret TEXT
            );
            CREATE TABLE IF NOT EXISTS telemetry_secrets (
                id INTEGER PRIMARY KEY CHECK(id=1), secret TEXT
            );",
        )?;
        Ok(database)
    }
}

impl SecretStore for LocalVault {
    fn get(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .open()?
            .query_row(
                "SELECT secret FROM provider_secrets WHERE id=?1",
                [id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    fn set(&self, id: &str, secret: Option<&str>) -> Result<()> {
        self.open()?.execute(
            "INSERT INTO provider_secrets(id,secret) VALUES(?1,?2)
             ON CONFLICT(id) DO UPDATE SET secret=excluded.secret",
            params![id, secret.filter(|value| !value.is_empty())],
        )?;
        Ok(())
    }

    fn get_telemetry_api_key(&self) -> Result<Option<String>> {
        Ok(self
            .open()?
            .query_row(
                "SELECT secret FROM telemetry_secrets WHERE id=1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    fn set_telemetry_api_key(&self, secret: Option<&str>) -> Result<()> {
        self.open()?.execute(
            "INSERT INTO telemetry_secrets(id,secret) VALUES(1,?1)
             ON CONFLICT(id) DO UPDATE SET secret=excluded.secret",
            [secret.filter(|value| !value.is_empty())],
        )?;
        Ok(())
    }
}

/// Explicitly opt-in test store. Credentials disappear with the process.
#[derive(Default)]
pub struct MemoryVault(Mutex<MemorySecrets>);

#[derive(Default)]
struct MemorySecrets {
    providers: HashMap<String, String>,
    telemetry: Option<String>,
}

impl SecretStore for MemoryVault {
    fn get(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .0
            .lock()
            .map_err(|_| SecretError::LockPoisoned)?
            .providers
            .get(id)
            .cloned())
    }

    fn set(&self, id: &str, secret: Option<&str>) -> Result<()> {
        let mut entries = self.0.lock().map_err(|_| SecretError::LockPoisoned)?;
        if let Some(value) = secret.filter(|value| !value.is_empty()) {
            entries.providers.insert(id.into(), value.into());
        } else {
            entries.providers.remove(id);
        }
        Ok(())
    }

    fn get_telemetry_api_key(&self) -> Result<Option<String>> {
        Ok(self
            .0
            .lock()
            .map_err(|_| SecretError::LockPoisoned)?
            .telemetry
            .clone())
    }

    fn set_telemetry_api_key(&self, secret: Option<&str>) -> Result<()> {
        self.0
            .lock()
            .map_err(|_| SecretError::LockPoisoned)?
            .telemetry = secret.filter(|value| !value.is_empty()).map(str::to_owned);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use anyhow::{Context, Result};
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn poisoned_credential_lock_is_typed_and_redacted() -> Result<()> {
        let vault = MemoryVault::default();
        vault.set("provider", Some("fixture-sensitive-key"))?;
        let panicked = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    if let Ok(_guard) = vault.0.lock() {
                        panic!("fixture lock failure");
                    }
                })
                .join()
        });
        assert!(panicked.is_err());
        for result in [
            vault.get("provider").map(|_| ()),
            vault.set("provider", None),
            vault.get_telemetry_api_key().map(|_| ()),
            vault.set_telemetry_api_key(None),
        ] {
            let error = result.err().context("poisoned credential lock must fail")?;
            assert!(matches!(error, SecretError::LockPoisoned));
            assert!(!format!("{error} {error:?}").contains("fixture-sensitive-key"));
        }
        Ok(())
    }

    #[test]
    fn unavailable_database_has_a_typed_source() -> Result<()> {
        let directory = TempDir::new()?;
        let vault = LocalVault::new(directory.path().join("missing/test.sqlite3"));
        let error = vault
            .get("provider")
            .err()
            .context("opening a missing parent directory must fail")?;
        assert!(matches!(error, SecretError::OpenDatabase { .. }));
        assert!(
            error
                .source()
                .and_then(|source| source.downcast_ref::<rusqlite::Error>())
                .is_some()
        );
        Ok(())
    }

    #[test]
    fn credential_write_errors_retain_sources_without_secret_values() -> Result<()> {
        let directory = TempDir::new()?;
        let path = directory.path().join("tandem.sqlite3");
        Connection::open(&path)?.execute_batch(
            "CREATE TABLE provider_secrets (id TEXT PRIMARY KEY, secret TEXT CHECK(secret IS NULL));
             CREATE TABLE telemetry_secrets (id INTEGER PRIMARY KEY, secret TEXT CHECK(secret IS NULL));"
        )?;
        let vault = LocalVault::new(path);
        for result in [
            vault.set("provider", Some("fixture-sensitive-key")),
            vault.set_telemetry_api_key(Some("fixture-sensitive-key")),
        ] {
            let error = result
                .err()
                .context("the fixture constraint must reject credentials")?;
            assert!(matches!(error, SecretError::Database(_)));
            let source = error
                .source()
                .context("database failure must retain its source")?;
            assert!(source.downcast_ref::<rusqlite::Error>().is_some());
            assert!(!format!("{error} {error:?} {source}").contains("fixture-sensitive-key"));
        }
        Ok(())
    }

    #[test]
    fn local_keys_and_clears_survive_restart() -> Result<()> {
        for value in [Some("fixture-replacement-key"), None, Some("")] {
            let directory = TempDir::new()?;
            let path = directory.path().join("tandem.sqlite3");
            let vault = LocalVault::new(path.clone());
            assert!(vault.get("provider")?.is_none());
            vault.set("provider", Some("fixture-initial-key"))?;
            vault.set("provider", value)?;
            drop(vault);
            let reopened = LocalVault::new(path.clone());
            let expected = value.filter(|value| !value.is_empty());
            assert_eq!(reopened.get("provider")?.as_deref(), expected);
            // Local persistence intentionally stores the user's key as plaintext.
            let persisted: Option<String> = Connection::open(path)?.query_row(
                "SELECT secret FROM provider_secrets WHERE id=?1",
                ["provider"],
                |row| row.get(0),
            )?;
            assert_eq!(persisted.as_deref(), expected);
        }
        Ok(())
    }

    #[test]
    fn telemetry_credentials_are_independent_and_clears_survive_restart() -> Result<()> {
        for value in [Some("fixture-replacement-tenant"), None, Some("")] {
            let directory = TempDir::new()?;
            let path = directory.path().join("tandem.sqlite3");
            let vault = LocalVault::new(path.clone());
            vault.set("telemetry", Some("fixture-provider-key"))?;
            vault.set_telemetry_api_key(Some("fixture-initial-tenant"))?;
            vault.set_telemetry_api_key(value)?;
            drop(vault);
            let reopened = LocalVault::new(path);
            assert_eq!(
                reopened.get_telemetry_api_key()?.as_deref(),
                value.filter(|value| !value.is_empty())
            );
            assert_eq!(
                reopened.get("telemetry")?.as_deref(),
                Some("fixture-provider-key")
            );
        }
        let memory = MemoryVault::default();
        memory.set("telemetry", Some("fixture-provider-key"))?;
        memory.set_telemetry_api_key(Some("fixture-tenant"))?;
        assert_eq!(
            memory.get("telemetry")?.as_deref(),
            Some("fixture-provider-key")
        );
        memory.set_telemetry_api_key(None)?;
        assert!(memory.get_telemetry_api_key()?.is_none());
        Ok(())
    }
}
