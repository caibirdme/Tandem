use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const DATABASE_FILE: &str = "tandem.sqlite3";
// Existing installations keep their database, WAL files, and SDK history in place.
const LEGACY_DATABASE_FILE: &str = "relay.sqlite3";
const LEGACY_APPLICATION_DIRECTORY: &str = "app.relay.desktop";

pub(crate) fn application_directory(preferred: &Path) -> Result<PathBuf> {
    if has_database(preferred)? {
        return Ok(preferred.to_owned());
    }
    let legacy = preferred.with_file_name(LEGACY_APPLICATION_DIRECTORY);
    if has_database(&legacy)? {
        return Ok(legacy);
    }
    Ok(preferred.to_owned())
}

fn has_database(directory: &Path) -> Result<bool> {
    for filename in [DATABASE_FILE, LEGACY_DATABASE_FILE] {
        if exists(&directory.join(filename))? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn database_path(preferred: &Path) -> Result<PathBuf> {
    if preferred
        .file_name()
        .is_some_and(|name| name == DATABASE_FILE)
        && !exists(preferred)?
    {
        let legacy = preferred.with_file_name(LEGACY_DATABASE_FILE);
        if exists(&legacy)? {
            return Ok(legacy);
        }
    }
    Ok(preferred.to_owned())
}

fn exists(path: &Path) -> Result<bool> {
    path.try_exists()
        .with_context(|| format!("无法检查应用数据路径：{}", path.display()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::{
        App, AppOptions, LocalVault, SecretStore,
        model::{Message, Preferences},
        store::Store,
    };

    #[test]
    fn fresh_install_uses_new_paths_without_creating_files() -> Result<()> {
        let directory = TempDir::new()?;
        let preferred = directory.path().join("app.tandem.desktop");
        assert_eq!(application_directory(&preferred)?, preferred);
        assert_eq!(
            database_path(&preferred.join(DATABASE_FILE))?,
            preferred.join(DATABASE_FILE)
        );
        assert!(!preferred.exists());
        Ok(())
    }

    #[test]
    fn new_database_wins_and_empty_new_directory_keeps_existing_data() -> Result<()> {
        for (new_database, old_database, expected_new) in [
            (None, None, true),
            (None, Some(LEGACY_DATABASE_FILE), false),
            (None, Some(DATABASE_FILE), false),
            (Some(DATABASE_FILE), Some(LEGACY_DATABASE_FILE), true),
            (Some(LEGACY_DATABASE_FILE), Some(LEGACY_DATABASE_FILE), true),
        ] {
            let directory = TempDir::new()?;
            let preferred = directory.path().join("app.tandem.desktop");
            let legacy = directory.path().join(LEGACY_APPLICATION_DIRECTORY);
            std::fs::create_dir(&preferred)?;
            std::fs::create_dir(&legacy)?;
            for (root, filename) in [(&preferred, new_database), (&legacy, old_database)] {
                if let Some(filename) = filename {
                    std::fs::write(root.join(filename), [])?;
                }
            }
            assert_eq!(
                application_directory(&preferred)?,
                if expected_new { preferred } else { legacy }
            );
        }
        Ok(())
    }

    #[test]
    fn explicit_directory_prefers_new_database_and_preserves_other_filenames() -> Result<()> {
        let directory = TempDir::new()?;
        let preferred = directory.path().join(DATABASE_FILE);
        let legacy = directory.path().join(LEGACY_DATABASE_FILE);
        std::fs::write(&legacy, [])?;
        assert_eq!(database_path(&preferred)?, legacy);
        std::fs::write(&preferred, [])?;
        assert_eq!(database_path(&preferred)?, preferred);
        let custom = directory.path().join("custom.sqlite3");
        assert_eq!(database_path(&custom)?, custom);
        Ok(())
    }

    fn seed_history(database: &Path) -> Result<()> {
        let store = Store::open(database)?;
        store.save_provider(&serde_json::from_value(json!({
            "id":"provider", "name":"Existing provider", "protocol":"anthropic",
            "baseUrl":"https://example.com", "models":[], "hasApiKey":true
        }))?)?;
        store.save_project(&serde_json::from_value(json!({
            "id":"project", "name":"Existing project", "path":"/workspace",
            "connection":{"kind":"local"}, "createdAt":"2026-10-19"
        }))?)?;
        store.save_session(&serde_json::from_value(json!({
            "id":"session", "title":"Existing conversation", "projectId":"project",
            "providerId":"provider", "modelId":"model", "permissionMode":"allow-all",
            "status":"idle", "createdAt":"2026-10-19", "updatedAt":"2026-10-19"
        }))?)?;
        for (id, role, content) in [
            ("user", "user", "Existing question"),
            ("reply", "assistant", "Existing answer"),
        ] {
            store.append_message(&serde_json::from_value::<Message>(json!({
                "id":id, "sessionId":"session", "role":role, "content":content,
                "createdAt":"2026-10-19"
            }))?)?;
        }
        store.save_preferences(&Preferences {
            cli_path: Some("existing-custom-cli".into()),
            ..Preferences::default()
        })?;
        LocalVault::new(database.to_owned()).set("provider", Some("fixture-existing-key"))?;
        Ok(())
    }

    #[test]
    fn legacy_database_credentials_and_sdk_history_are_reused_together() -> Result<()> {
        let directory = TempDir::new()?;
        let preferred = directory.path().join("app.tandem.desktop");
        let legacy = directory.path().join(LEGACY_APPLICATION_DIRECTORY);
        std::fs::create_dir_all(legacy.join("runtime/session-state/session"))?;
        let sdk_history = legacy.join("runtime/session-state/session/history.jsonl");
        std::fs::write(&sdk_history, "existing SDK history")?;
        seed_history(&legacy.join(LEGACY_DATABASE_FILE))?;

        let selected = application_directory(&preferred)?;
        let app = App::open(AppOptions::local(selected.clone()))?;
        assert_eq!(selected, legacy);
        assert_eq!(app.store.provider("provider")?.name, "Existing provider");
        assert_eq!(app.store.project("project")?.name, "Existing project");
        assert_eq!(app.store.session("session")?.title, "Existing conversation");
        assert_eq!(
            app.store.messages("session")?[0].content,
            "Existing question"
        );
        assert_eq!(app.store.messages("session")?[1].content, "Existing answer");
        assert_eq!(
            app.store.preferences()?.cli_path.as_deref(),
            Some("existing-custom-cli")
        );
        assert_eq!(
            app.options.secrets.get("provider")?.as_deref(),
            Some("fixture-existing-key")
        );
        assert_eq!(
            std::fs::read_to_string(sdk_history)?,
            "existing SDK history"
        );
        app.options
            .secrets
            .set("provider", Some("fixture-updated-key"))?;
        assert_eq!(
            LocalVault::new(legacy.join(LEGACY_DATABASE_FILE))
                .get("provider")?
                .as_deref(),
            Some("fixture-updated-key")
        );
        assert!(!preferred.exists());
        assert!(!legacy.join(DATABASE_FILE).exists());
        Ok(())
    }

    #[test]
    fn new_database_and_credentials_take_precedence_together() -> Result<()> {
        let directory = TempDir::new()?;
        let legacy = directory.path().join(LEGACY_DATABASE_FILE);
        let preferred = directory.path().join(DATABASE_FILE);
        seed_history(&legacy)?;
        std::fs::write(&preferred, [])?;
        seed_history(&preferred)?;
        let vault = LocalVault::new(preferred);
        vault.set("provider", Some("fixture-new-key"))?;
        let app = App::open(AppOptions::local(directory.path().to_owned()))?;
        assert_eq!(app.store.messages("session")?.len(), 2);
        assert_eq!(
            app.options.secrets.get("provider")?.as_deref(),
            Some("fixture-new-key")
        );
        assert_eq!(
            LocalVault::new(legacy).get("provider")?.as_deref(),
            Some("fixture-existing-key")
        );
        Ok(())
    }
}
