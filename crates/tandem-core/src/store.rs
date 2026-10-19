use std::{path::Path, sync::Mutex};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::de::DeserializeOwned;

use crate::{
    model::{Message, Preferences, Project, Provider, Session, SessionStatus},
    storage_paths::database_path,
};

pub(crate) struct Store {
    connection: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(database_path(path)?).context("无法打开本地数据文件")?;
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
        Ok(store)
    }

    fn with<T>(&self, operation: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("数据锁已损坏，请重启应用"))?;
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
            .context("供应商不存在，请在设置中重新选择")
    }

    pub fn optional_provider(&self, id: &str) -> Result<Option<Provider>> {
        self.lookup("SELECT data FROM providers WHERE id=?1", id)
    }

    pub fn project(&self, id: &str) -> Result<Project> {
        self.lookup("SELECT data FROM projects WHERE id=?1", id)?
            .context("项目不存在")
    }

    pub fn session(&self, id: &str) -> Result<Session> {
        self.lookup("SELECT data FROM sessions WHERE id=?1", id)?
            .context("对话不存在")
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
                bail!("该供应商仍有关联对话，请先删除这些对话");
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
                .context("对话不存在")?;
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

    pub fn append_message(&self, message: &Message) -> Result<()> {
        self.with(|db| {
            db.execute(
                "INSERT INTO messages(id,session_id,data) VALUES(?1,?2,?3)",
                params![
                    message.id,
                    message.session_id,
                    serde_json::to_string(message)?
                ],
            )?;
            Ok(())
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
                db.prepare("SELECT data FROM messages WHERE session_id=?1 ORDER BY rowid")?;
            query
                .query_map([session_id], |row| row.get::<_, String>(0))?
                .map(|row| Ok(serde_json::from_str(&row?)?))
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

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
            "modelId":"model", "status":"idle", "createdAt":"2026-10-17", "updatedAt":"2026-10-17"
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
        assert!(store.provider("missing").is_err());
        assert!(store.project("missing").is_err());
        assert!(store.session("missing").is_err());
        assert!(store.delete_provider("provider").is_err());
        Ok(())
    }
}
