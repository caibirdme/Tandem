use std::{path::Path, sync::Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use serde::de::DeserializeOwned;

use crate::{
    activities::interrupt_activity,
    model::{Message, MessageRole, Preferences, Project, Provider, Session, SessionStatus},
};

/// Local persistence failures and missing records that callers can handle by type.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("Cannot open the local data file: {source}")]
    OpenDatabase {
        #[source]
        source: rusqlite::Error,
    },
    #[error("Cannot access local data: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("Cannot encode or decode local data: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Data lock is unavailable. Restart the application.")]
    LockPoisoned,
    #[error("Provider does not exist. Select a provider again in Settings.")]
    ProviderNotFound { id: String },
    #[error("Project does not exist.")]
    ProjectNotFound { id: String },
    #[error("Conversation does not exist.")]
    SessionNotFound { id: String },
    #[error("Delete this provider's conversations before deleting the provider.")]
    ProviderInUse { id: String },
    #[error("Cannot update activity {id}: its conversation or message role does not match.")]
    ActivityConflict { id: String },
}

type Result<T> = std::result::Result<T, StorageError>;

pub(crate) struct Store {
    connection: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let connection =
            Connection::open(path).map_err(|source| StorageError::OpenDatabase { source })?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS preferences (
                 id INTEGER PRIMARY KEY CHECK(id=1), data TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS providers (id TEXT PRIMARY KEY, data TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS projects (id TEXT PRIMARY KEY, data TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS sessions (
                 id TEXT PRIMARY KEY,
                 project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
                 provider_id TEXT NOT NULL REFERENCES providers(id),
                 data TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS messages (
                 id TEXT PRIMARY KEY,
                 session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                 data TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS messages_by_session ON messages(session_id);
             PRAGMA user_version=1;",
        )?;
        let store = Self {
            connection: Mutex::new(connection),
        };
        for mut session in store.sessions()? {
            if session.status == SessionStatus::Running {
                session.status = SessionStatus::Idle;
                store.save_session(&session)?;
            }
        }
        for mut message in store.list::<Message>(
            "SELECT data FROM messages WHERE json_extract(data,'$.role')='activity'
             AND json_extract(data,'$.activity.status')='running'",
        )? {
            if message.activity.as_mut().is_some_and(interrupt_activity) {
                store.save_activity(&message)?;
            }
        }
        Ok(store)
    }

    fn with<T>(&self, operation: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        operation(&connection)
    }

    pub fn preferences(&self) -> Result<Preferences> {
        self.with(|db| {
            let data: Option<String> = db
                .query_row("SELECT data FROM preferences WHERE id=1", [], |row| {
                    row.get(0)
                })
                .optional()?;
            data.map_or_else(
                || Ok(Preferences::default()),
                |json| Ok(serde_json::from_str(&json)?),
            )
        })
    }

    pub fn save_preferences(&self, preferences: &Preferences) -> Result<()> {
        self.with(|db| {
            db.execute(
                "INSERT INTO preferences(id,data) VALUES(1,?1)
                 ON CONFLICT(id) DO UPDATE SET data=excluded.data",
                [serde_json::to_string(preferences)?],
            )?;
            Ok(())
        })
    }

    fn list<T: DeserializeOwned>(&self, sql: &str) -> Result<Vec<T>> {
        self.with(|db| {
            let mut query = db.prepare(sql)?;
            query
                .query_map([], |row| row.get::<_, String>(0))?
                .map(|row| Ok(serde_json::from_str(&row?)?))
                .collect()
        })
    }

    pub fn providers(&self) -> Result<Vec<Provider>> {
        self.list("SELECT data FROM providers ORDER BY rowid")
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        self.list("SELECT data FROM projects ORDER BY rowid")
    }

    pub fn sessions(&self) -> Result<Vec<Session>> {
        let mut sessions: Vec<Session> = self.list("SELECT data FROM sessions")?;
        sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        Ok(sessions)
    }

    pub fn provider(&self, id: &str) -> Result<Provider> {
        self.optional_provider(id)?
            .ok_or_else(|| StorageError::ProviderNotFound { id: id.into() })
    }

    pub fn optional_provider(&self, id: &str) -> Result<Option<Provider>> {
        self.lookup("SELECT data FROM providers WHERE id=?1", id)
    }

    pub fn project(&self, id: &str) -> Result<Project> {
        self.lookup("SELECT data FROM projects WHERE id=?1", id)?
            .ok_or_else(|| StorageError::ProjectNotFound { id: id.into() })
    }

    pub fn session(&self, id: &str) -> Result<Session> {
        self.lookup("SELECT data FROM sessions WHERE id=?1", id)?
            .ok_or_else(|| StorageError::SessionNotFound { id: id.into() })
    }

    fn lookup<T: DeserializeOwned>(&self, sql: &str, id: &str) -> Result<Option<T>> {
        self.with(|db| {
            let data: Option<String> = db.query_row(sql, [id], |row| row.get(0)).optional()?;
            data.map(|json| serde_json::from_str(&json).map_err(Into::into))
                .transpose()
        })
    }

    pub fn has_messages(&self, session_id: &str) -> Result<bool> {
        self.with(|db| {
            Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE session_id=?1)",
                [session_id],
                |row| row.get(0),
            )?)
        })
    }

    pub fn save_provider(&self, provider: &Provider) -> Result<()> {
        self.with(|db| {
            db.execute(
                "INSERT INTO providers(id,data) VALUES(?1,?2)
                 ON CONFLICT(id) DO UPDATE SET data=excluded.data",
                params![provider.id, serde_json::to_string(provider)?],
            )?;
            Ok(())
        })
    }

    pub fn delete_provider(&self, id: &str) -> Result<()> {
        self.with(|db| {
            let referenced: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE provider_id=?1)",
                [id],
                |row| row.get(0),
            )?;
            if referenced {
                return Err(StorageError::ProviderInUse { id: id.into() });
            }
            db.execute("DELETE FROM providers WHERE id=?1", [id])?;
            Ok(())
        })
    }

    pub fn save_project(&self, project: &Project) -> Result<()> {
        self.with(|db| {
            db.execute(
                "INSERT INTO projects(id,data) VALUES(?1,?2)
                 ON CONFLICT(id) DO UPDATE SET data=excluded.data",
                params![project.id, serde_json::to_string(project)?],
            )?;
            Ok(())
        })
    }

    pub fn delete_project(&self, id: &str) -> Result<()> {
        self.with(|db| {
            db.execute("DELETE FROM projects WHERE id=?1", [id])?;
            Ok(())
        })
    }

    pub fn save_session(&self, session: &Session) -> Result<()> {
        self.with(|db| {
            db.execute(
                "INSERT INTO sessions(id,project_id,provider_id,data) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(id) DO UPDATE SET project_id=excluded.project_id,
                 provider_id=excluded.provider_id,data=excluded.data",
                params![
                    session.id,
                    session.project_id,
                    session.provider_id,
                    serde_json::to_string(session)?
                ],
            )?;
            Ok(())
        })
    }

    pub fn update_session(&self, id: &str, update: impl FnOnce(&mut Session)) -> Result<Session> {
        self.with(|db| {
            let data: String = db
                .query_row("SELECT data FROM sessions WHERE id=?1", [id], |row| {
                    row.get(0)
                })
                .optional()?
                .ok_or_else(|| StorageError::SessionNotFound { id: id.into() })?;
            let mut session: Session = serde_json::from_str(&data)?;
            update(&mut session);
            db.execute(
                "UPDATE sessions SET project_id=?2,provider_id=?3,data=?4 WHERE id=?1",
                params![
                    id,
                    session.project_id,
                    session.provider_id,
                    serde_json::to_string(&session)?
                ],
            )?;
            Ok(session)
        })
    }

    pub fn delete_session(&self, id: &str) -> Result<()> {
        self.with(|db| {
            db.execute("DELETE FROM sessions WHERE id=?1", [id])?;
            Ok(())
        })
    }

    pub fn append_message(&self, message: &Message) -> Result<Message> {
        self.with(|db| {
            db.execute(
                "INSERT INTO messages(id,session_id,data) VALUES(?1,?2,?3)",
                params![
                    message.id,
                    message.session_id,
                    serde_json::to_string(message)?
                ],
            )?;
            let mut saved = message.clone();
            saved.sequence = Some(db.query_row(
                "SELECT rowid FROM messages WHERE id=?1",
                [&message.id],
                |row| row.get(0),
            )?);
            Ok(saved)
        })
    }

    pub fn save_activity(&self, message: &Message) -> Result<Message> {
        if message.role != MessageRole::Activity || message.activity.is_none() {
            return Err(StorageError::ActivityConflict {
                id: message.id.clone(),
            });
        }
        self.with(|db| {
            let changed = db.execute(
                "INSERT INTO messages(id,session_id,data) VALUES(?1,?2,?3)
                 ON CONFLICT(id) DO UPDATE SET data=excluded.data
                 WHERE messages.session_id=excluded.session_id
                 AND json_extract(messages.data,'$.role') IN ('assistant','activity')
                 AND json_extract(excluded.data,'$.role')='activity'",
                params![
                    message.id,
                    message.session_id,
                    serde_json::to_string(message)?
                ],
            )?;
            if changed == 0 {
                return Err(StorageError::ActivityConflict {
                    id: message.id.clone(),
                });
            }
            let mut saved = message.clone();
            saved.sequence = Some(db.query_row(
                "SELECT rowid FROM messages WHERE id=?1 AND session_id=?2",
                [&message.id, &message.session_id],
                |row| row.get(0),
            )?);
            Ok(saved)
        })
    }

    /// Select the first complete turn's final reply, bounded by user messages.
    /// SQL truncates each text before decoding; tool results are never loaded.
    pub fn title_exchange(&self, session_id: &str) -> Result<Option<(String, String)>> {
        self.with(|db| {
            let mut query = db.prepare(
                "SELECT json_extract(data,'$.role'),
                        substr(json_extract(data,'$.content'),1,6000),
                        coalesce(json_extract(data,'$.interrupted'),0)
                 FROM messages WHERE session_id=?1
                 AND json_extract(data,'$.role') IN ('user','assistant') ORDER BY rowid",
            )?;
            let mut rows = query.query([session_id])?;
            let mut user = None;
            let mut assistant = None;
            while let Some(row) = rows.next()? {
                let role: String = row.get(0)?;
                let content: String = row.get(1)?;
                let interrupted: bool = row.get(2)?;
                if role == "user" {
                    if let Some(exchange) = user.take().zip(assistant.take()) {
                        return Ok(Some(exchange));
                    }
                    user = Some(content);
                } else if user.is_some() {
                    // A later interrupted/empty reply invalidates earlier progress
                    // text in this turn instead of falling back to a tool preamble.
                    assistant = (!interrupted && !content.trim().is_empty()).then_some(content);
                }
            }
            Ok(user.zip(assistant))
        })
    }

    pub fn messages(&self, session_id: &str) -> Result<Vec<Message>> {
        self.session(session_id)?;
        self.with(|db| {
            let mut query =
                db.prepare("SELECT rowid,data FROM messages WHERE session_id=?1 ORDER BY rowid")?;
            query
                .query_map([session_id], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })?
                .map(|row| {
                    let (sequence, data) = row?;
                    let mut message: Message = serde_json::from_str(&data)?;
                    message.sequence = Some(sequence);
                    Ok(message)
                })
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use anyhow::{Context, Result};
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn unavailable_database_retains_its_sqlite_source() -> Result<()> {
        let directory = TempDir::new()?;
        let result = Store::open(&directory.path().join("missing/test.sqlite3"));
        let Err(error) = result else {
            anyhow::bail!("opening a missing parent directory must fail");
        };
        assert!(matches!(error, StorageError::OpenDatabase { .. }));
        assert!(
            error
                .source()
                .and_then(|source| source.downcast_ref::<rusqlite::Error>())
                .is_some()
        );
        Ok(())
    }

    #[test]
    fn primary_key_reads_do_not_decode_unrelated_corrupt_records() -> Result<()> {
        let directory = TempDir::new()?;
        let store = Store::open(&directory.path().join("test.sqlite3"))?;
        store.save_provider(&serde_json::from_value(json!({
            "id":"provider", "name":"Provider", "protocol":"anthropic",
            "baseUrl":"https://example.com", "models":[]
        }))?)?;
        store.save_project(&serde_json::from_value(json!({
            "id":"project", "name":"Project", "path":"/workspace",
            "connection":{"kind":"local"}, "createdAt":"2026-10-17"
        }))?)?;
        store.save_session(&serde_json::from_value(json!({
            "id":"session", "title":"Session", "projectId":"project", "providerId":"provider",
            "modelId":"model", "status":"idle", "titleState":"pending", "permissionMode":"ask",
            "createdAt":"2026-10-17", "updatedAt":"2026-10-17"
        }))?)?;
        store.with(|db| {
            db.execute("INSERT INTO providers(id,data) VALUES('broken','invalid json')", [])?;
            db.execute("INSERT INTO projects(id,data) VALUES('broken','invalid json')", [])?;
            db.execute("INSERT INTO sessions(id,provider_id,data) VALUES('broken','provider','invalid json')", [])?;
            Ok(())
        })?;
        assert_eq!(store.provider("provider")?.id, "provider");
        assert_eq!(store.project("project")?.id, "project");
        assert_eq!(store.session("session")?.id, "session");
        assert!(!store.has_messages("session")?);
        assert!(matches!(
            store.provider("missing"),
            Err(StorageError::ProviderNotFound { id }) if id == "missing"
        ));
        assert!(matches!(
            store.project("missing"),
            Err(StorageError::ProjectNotFound { id }) if id == "missing"
        ));
        assert!(matches!(
            store.session("missing"),
            Err(StorageError::SessionNotFound { id }) if id == "missing"
        ));
        assert!(matches!(
            store.update_session("missing", |_| {}),
            Err(StorageError::SessionNotFound { id }) if id == "missing"
        ));
        assert!(matches!(
            store.delete_provider("provider"),
            Err(StorageError::ProviderInUse { id }) if id == "provider"
        ));
        let error = store
            .provider("broken")
            .err()
            .context("corrupt provider must fail decoding")?;
        assert!(matches!(error, StorageError::Json(_)));
        assert!(
            error
                .source()
                .and_then(|source| source.downcast_ref::<serde_json::Error>())
                .is_some()
        );
        Ok(())
    }
}
