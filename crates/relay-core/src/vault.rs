use std::{collections::HashMap, path::PathBuf, sync::Mutex, time::Duration};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

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
        let database = Connection::open(&self.database).context("无法打开本地 API Key 数据库")?;
        database.busy_timeout(Duration::from_secs(5))?;
        database.execute_batch(
            "CREATE TABLE IF NOT EXISTS provider_secrets (
                id TEXT PRIMARY KEY, secret TEXT
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
}

/// Explicitly opt-in test store. Credentials disappear with the process.
#[derive(Default)]
pub struct MemoryVault(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryVault {
    fn get(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("credential lock poisoned"))?
            .get(id)
            .cloned())
    }

    fn set(&self, id: &str, secret: Option<&str>) -> Result<()> {
        let mut entries = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("credential lock poisoned"))?;
        if let Some(value) = secret.filter(|value| !value.is_empty()) {
            entries.insert(id.into(), value.into());
        } else {
            entries.remove(id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn local_keys_and_clears_survive_restart() -> Result<()> {
        for value in [Some("fixture-replacement-key"), None, Some("")] {
            let directory = TempDir::new()?;
            let path = directory.path().join("relay.sqlite3");
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
}
