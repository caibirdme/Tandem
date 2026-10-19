use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tandem_remote::{RemoteClient, SshConnection};
use uuid::Uuid;

use crate::{
    App,
    app::now,
    model::{Connection, DirectoryEntry, DirectoryListing, Project, SessionStatus, validate_name},
};

impl App {
    pub(crate) async fn create_project(
        &self,
        name: String,
        path: String,
        connection: Connection,
    ) -> Result<Project> {
        validate_name(&name, "Project name")?;
        let path = match &connection {
            Connection::Local => {
                let canonical = tokio::fs::canonicalize(expand_home(&path)?)
                    .await
                    .context("The directory does not exist or cannot be accessed")?;
                if !tokio::fs::metadata(&canonical).await?.is_dir() {
                    bail!("The project path must be a directory.");
                }
                canonical.to_string_lossy().into_owned()
            }
            Connection::Ssh { .. } => {
                self.remote_client(&connection)?
                    .inspect_directory(&path)
                    .await?
                    .path
            }
        };
        let project = Project {
            id: Uuid::new_v4().to_string(),
            name: name.trim().into(),
            path,
            connection,
            created_at: now(),
        };
        self.store.save_project(&project)?;
        Ok(project)
    }

    pub(crate) fn remote_client(&self, connection: &Connection) -> Result<RemoteClient> {
        let Connection::Ssh {
            host,
            port,
            user,
            identity_file,
        } = connection
        else {
            bail!("This project does not have an SSH connection.");
        };
        Ok(RemoteClient::new(
            SshConnection {
                host: host.clone(),
                port: *port,
                user: user.clone(),
                identity_file: identity_file.clone(),
            },
            self.options.remote_artifacts.clone(),
        )?)
    }

    pub(crate) async fn browse_directory(
        &self,
        connection: &Connection,
        path: &str,
    ) -> Result<DirectoryListing> {
        if matches!(connection, Connection::Ssh { .. }) {
            let client = self.remote_client(connection)?;
            let directory = client.inspect_directory(path).await?;
            let entries = client.list_directories(&directory.path).await?;
            return Ok(DirectoryListing {
                path: directory.path,
                entries: entries
                    .into_iter()
                    .map(|entry| DirectoryEntry {
                        name: entry.name,
                        path: entry.path,
                    })
                    .collect(),
            });
        }
        let path = tokio::fs::canonicalize(expand_home(path)?)
            .await
            .context("The directory does not exist or cannot be accessed")?;
        let mut directory = tokio::fs::read_dir(&path).await?;
        let mut entries = Vec::new();
        while let Some(entry) = directory.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                entries.push(DirectoryEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    path: entry.path().to_string_lossy().into_owned(),
                });
            }
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(DirectoryListing {
            path: path.to_string_lossy().into_owned(),
            entries,
        })
    }

    pub(crate) async fn delete_project(&self, id: &str) -> Result<()> {
        let _guard = self.mutation_gate.lock().await;
        let sessions = self.store.sessions()?;
        let project_sessions: Vec<_> = sessions
            .iter()
            .filter(|session| session.project_id.as_deref() == Some(id))
            .collect();
        if project_sessions
            .iter()
            .any(|session| session.status == SessionStatus::Running)
        {
            bail!("Stop the project's running work before continuing.");
        }
        for session in &project_sessions {
            self.disconnect_session(&session.id).await;
        }
        let mut titles = self.titles.lock()?;
        self.store.delete_project(id)?;
        for session in project_sessions {
            titles.cancel(&session.id);
        }
        Ok(())
    }

    pub(crate) async fn check_connection(&self, connection: &Connection) -> Result<String> {
        if matches!(connection, Connection::Local) {
            return Ok("Local connection is ready".into());
        }
        let report = self.remote_client(connection)?.bootstrap().await?;
        Ok(format!(
            "SSH connected; remote service is ready ({} / {})",
            report.platform.os, report.platform.architecture
        ))
    }
}

fn expand_home(path: &str) -> Result<PathBuf> {
    if path == "~" || path.starts_with("~/") || path.is_empty() {
        let home = dirs::home_dir().context("Cannot determine the user's home directory")?;
        Ok(home.join(path.trim_start_matches('~').trim_start_matches('/')))
    } else {
        Ok(PathBuf::from(path))
    }
}
