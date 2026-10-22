use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use copilot_runtime::workspace::{
    ReviewFileDiff, ReviewFilePage, ReviewSelection, ReviewSummary, Snapshot, WorkspaceComparison,
    WorkspaceReview,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tandem_remote::RemoteTransport;
use tokio::io::AsyncReadExt;

use crate::{
    App, Error, Result,
    agent::AgentHandle,
    model::{Connection, Session},
};

const MAX_CACHE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone)]
pub(super) enum WorkspaceHandle {
    Local(Arc<WorkspaceReview>),
    Remote {
        transport: Arc<RemoteTransport>,
        workspace_id: String,
    },
}

impl WorkspaceHandle {
    fn connected(&self) -> bool {
        match self {
            Self::Local(_) => true,
            Self::Remote { transport, .. } => !transport.is_closed(),
        }
    }

    async fn request<T: DeserializeOwned>(&self, command: &str, mut payload: Value) -> Result<T> {
        let Self::Remote {
            transport,
            workspace_id,
        } = self
        else {
            return Err(Error::ReviewNotRegistered);
        };
        payload["workspaceId"] = json!(workspace_id);
        let result = remote_request(transport, command, payload).await?;
        serde_json::from_value(result).map_err(|source| Error::InvalidReviewData(source.into()))
    }

    pub(super) async fn capture(&self, previous: Option<&str>) -> Result<Snapshot> {
        match self {
            Self::Local(workspace) => Ok(workspace.capture(previous).await?),
            Self::Remote { .. } => {
                self.request("capture_review_snapshot", json!({"previous":previous}))
                    .await
            }
        }
    }

    pub(super) async fn compare(
        &self,
        before: &str,
        after: &str,
        selection: ReviewSelection,
    ) -> Result<ReviewSummary> {
        match self {
            Self::Local(workspace) => Ok(workspace.compare(before, after, selection).await?),
            Self::Remote { .. } => {
                self.request(
                    "compare_review_snapshots",
                    json!({"before":before,"after":after,"selection":selection}),
                )
                .await
            }
        }
    }

    pub(super) async fn workspace(&self, comparison: WorkspaceComparison) -> Result<ReviewSummary> {
        match self {
            Self::Local(workspace) => Ok(workspace.workspace(comparison).await?),
            Self::Remote { .. } => {
                self.request("get_workspace_review", json!({"comparison":comparison}))
                    .await
            }
        }
    }

    pub(super) async fn files(
        &self,
        review_id: &str,
        revision: &str,
        cursor: usize,
    ) -> Result<ReviewFilePage> {
        match self {
            Self::Local(workspace) => Ok(workspace.files(review_id, revision, cursor).await?),
            Self::Remote { .. } => {
                self.request(
                    "list_review_files",
                    json!({"reviewId":review_id,"revision":revision,"cursor":cursor}),
                )
                .await
            }
        }
    }

    pub(super) async fn file(
        &self,
        review_id: &str,
        revision: &str,
        file_id: &str,
    ) -> Result<ReviewFileDiff> {
        match self {
            Self::Local(workspace) => Ok(workspace.file(review_id, revision, file_id).await?),
            Self::Remote { .. } => {
                self.request(
                    "get_review_file",
                    json!({"reviewId":review_id,"revision":revision,"fileId":file_id}),
                )
                .await
            }
        }
    }
}

impl App {
    pub(crate) async fn shutdown_review_locations(&self) {
        let locations = std::mem::take(&mut *self.reviews.locations.lock().await);
        for handle in locations.into_values() {
            if let WorkspaceHandle::Remote { transport, .. } = handle {
                transport.shutdown().await;
            }
        }
    }

    pub(crate) async fn cleanup_review_location(&self, session_id: &str) {
        if let Ok(session) = self.store.session(session_id)
            && let Some(project_id) = session.project_id
        {
            self.reviews.locations.lock().await.remove(&project_id);
        }
    }

    pub(super) async fn review_location(
        &self,
        session: &Session,
        agent: Option<&AgentHandle>,
    ) -> Result<WorkspaceHandle> {
        let project_id = session
            .project_id
            .as_deref()
            .ok_or(Error::WorkSessionRequired)?;
        let project = self.store.project(project_id)?;
        if agent.is_none() {
            let locations = self.reviews.locations.lock().await;
            if let Some(handle) = locations
                .get(project_id)
                .filter(|handle| handle.connected())
            {
                return Ok(handle.clone());
            }
        }
        let handle = match &project.connection {
            Connection::Local => WorkspaceHandle::Local(Arc::new(WorkspaceReview::new(
                PathBuf::from(project.path),
                self.options
                    .data_directory
                    .join("reviews")
                    .join("workspaces")
                    .join(project_id),
            ))),
            Connection::Ssh { .. } => {
                let transport = match agent {
                    Some(AgentHandle::Remote { transport, .. }) => transport.clone(),
                    _ => Arc::new(
                        self.remote_client(&project.connection)?
                            .connect_review()
                            .await?,
                    ),
                };
                remote_request(
                    &transport,
                    "open_review_workspace",
                    json!({"workspaceId":project_id,"directory":project.path}),
                )
                .await?;
                WorkspaceHandle::Remote {
                    transport,
                    workspace_id: project_id.into(),
                }
            }
        };
        self.reviews
            .locations
            .lock()
            .await
            .insert(project_id.into(), handle.clone());
        Ok(handle)
    }

    pub(super) fn review_cache(
        &self,
        review_id: &str,
        revision: &str,
        item: &str,
    ) -> Result<PathBuf> {
        validate_id(review_id)?;
        validate_id(revision)?;
        validate_id(item)?;
        Ok(self
            .options
            .data_directory
            .join("reviews")
            .join("cache")
            .join(review_id)
            .join(revision)
            .join(format!("{item}.json")))
    }
}

async fn remote_request(
    transport: &Arc<RemoteTransport>,
    command: &str,
    payload: Value,
) -> Result<Value> {
    let transport = transport.clone();
    let command = command.to_owned();
    // Canceling the caller's review budget must not close a shared agent transport.
    // The registered request retains its own bounded transport deadline.
    tokio::spawn(async move { transport.request(&command, payload).await })
        .await
        .map_err(Error::ReviewTask)?
        .map_err(Into::into)
}

pub(super) fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 160
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(Error::Review(
            copilot_runtime::workspace::ReviewError::InvalidInput,
        ));
    }
    Ok(())
}

pub(super) async fn cached<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if file.metadata().await?.len() > MAX_CACHE_BYTES {
        return Err(copilot_runtime::workspace::ReviewError::Limit.into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_CACHE_BYTES + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() as u64 > MAX_CACHE_BYTES {
        return Err(copilot_runtime::workspace::ReviewError::Limit.into());
    }
    Ok(Some(serde_json::from_slice(&bytes).map_err(|source| {
        Error::InvalidReviewData(source.into())
    })?))
}

pub(super) async fn cache(path: &Path, value: &impl Serialize) -> Result<()> {
    if tokio::fs::try_exists(path).await? {
        return Ok(());
    }
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() as u64 > MAX_CACHE_BYTES {
        return Err(copilot_runtime::workspace::ReviewError::Limit.into());
    }
    let parent = path.parent().ok_or(Error::ReviewNotRegistered)?;
    tokio::fs::create_dir_all(parent).await?;
    let temporary = parent.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    tokio::fs::write(&temporary, bytes).await?;
    match tokio::fs::rename(&temporary, path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            tokio::fs::remove_file(temporary).await?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_closes_review_only_transports_without_an_agent_session() -> anyhow::Result<()>
    {
        let directory = tempfile::TempDir::new()?;
        let app = App::open(crate::AppOptions::local(directory.path().to_path_buf()))?;
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "while read request; do :; done"]);
        let transport = Arc::new(RemoteTransport::spawn(command)?);
        app.reviews.locations.lock().await.insert(
            "project".into(),
            WorkspaceHandle::Remote {
                transport: transport.clone(),
                workspace_id: "project".into(),
            },
        );
        app.shutdown().await?;
        assert!(transport.is_closed());
        assert!(app.reviews.locations.lock().await.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn canceled_review_waiter_does_not_close_shared_agent_transport() -> anyhow::Result<()> {
        let mut command = tokio::process::Command::new("sh");
        command.args([
            "-c",
            r#"
read request
printf '%s\n' '{"event":{"kind":"review_started"}}'
read request
printf '%s\n' '{"id":1,"value":null}'
printf '%s\n' '{"id":2,"value":"ready"}'
while read request; do :; done
"#,
        ]);
        let transport = Arc::new(RemoteTransport::spawn(command)?);
        let mut events = transport.subscribe();
        let review_transport = transport.clone();
        let waiter = tokio::spawn(async move {
            remote_request(&review_transport, "capture_review_snapshot", Value::Null).await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), events.recv()).await??;
        waiter.abort();
        assert!(waiter.await.is_err());
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            transport.request("ping", Value::Null),
        )
        .await??;
        assert_eq!(result, json!("ready"));
        assert!(!transport.is_closed());
        transport.shutdown().await;
        Ok(())
    }
}
