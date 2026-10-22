use std::{collections::HashMap, path::PathBuf, sync::Arc};

use copilot_runtime::workspace::{ReviewSelection, WorkspaceComparison, WorkspaceReview};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::required;
use crate::error::{Result, ServerError};

struct Binding {
    directory: PathBuf,
    workspace: Arc<WorkspaceReview>,
}

/// Workspace bindings cannot be used from a different proxy connection.
#[derive(Default)]
pub(super) struct ReviewHost {
    bindings: Mutex<HashMap<(u64, String), Binding>>,
}

impl ReviewHost {
    pub(super) async fn disconnect(&self, owner: u64) {
        self.bindings
            .lock()
            .await
            .retain(|(connection, _), _| *connection != owner);
    }

    pub(super) async fn dispatch(
        &self,
        owner: u64,
        command: &str,
        payload: &Value,
    ) -> Result<Value> {
        let id = required(payload, "workspaceId")?;
        validate_id(id)?;
        if command == "open_review_workspace" {
            return self.open(owner, id, payload).await;
        }
        let workspace = self
            .bindings
            .lock()
            .await
            .get(&(owner, id.to_owned()))
            .map(|binding| binding.workspace.clone())
            .ok_or(ServerError::ReviewNotBound)?;
        match command {
            "capture_review_snapshot" => {
                let previous = match &payload["previous"] {
                    Value::Null => None,
                    Value::String(value) => Some(value.as_str()),
                    _ => return Err(ServerError::RequiredField { field: "previous" }),
                };
                Ok(json!(workspace.capture(previous).await?))
            }
            "compare_review_snapshots" => {
                let selection: ReviewSelection =
                    serde_json::from_value(payload["selection"].clone()).map_err(|source| {
                        ServerError::InvalidPayload {
                            field: "selection",
                            source: source.into(),
                        }
                    })?;
                Ok(json!(
                    workspace
                        .compare(
                            required(payload, "before")?,
                            required(payload, "after")?,
                            selection
                        )
                        .await?
                ))
            }
            "get_workspace_review" => {
                let comparison: WorkspaceComparison =
                    serde_json::from_value(payload["comparison"].clone()).map_err(|source| {
                        ServerError::InvalidPayload {
                            field: "comparison",
                            source: source.into(),
                        }
                    })?;
                Ok(json!(workspace.workspace(comparison).await?))
            }
            "list_review_files" => {
                let cursor = match payload.get("cursor") {
                    None | Some(Value::Null) => 0,
                    Some(value) => value
                        .as_u64()
                        .and_then(|value| usize::try_from(value).ok())
                        .ok_or(ServerError::InvalidReviewCursor)?,
                };
                Ok(json!(
                    workspace
                        .files(
                            required(payload, "reviewId")?,
                            required(payload, "revision")?,
                            cursor
                        )
                        .await?
                ))
            }
            "get_review_file" => Ok(json!(
                workspace
                    .file(
                        required(payload, "reviewId")?,
                        required(payload, "revision")?,
                        required(payload, "fileId")?,
                    )
                    .await?
            )),
            _ => Err(ServerError::UnsupportedCommand),
        }
    }

    async fn open(&self, owner: u64, id: &str, payload: &Value) -> Result<Value> {
        let directory = tokio::fs::canonicalize(required(payload, "directory")?)
            .await
            .map_err(copilot_runtime::workspace::ReviewError::WorkspaceUnavailable)?;
        if !tokio::fs::metadata(&directory).await?.is_dir() {
            return Err(ServerError::InvalidReviewWorkspace);
        }
        let mut bindings = self.bindings.lock().await;
        let key = (owner, id.to_owned());
        if let Some(binding) = bindings.get(&key) {
            if binding.directory != directory {
                return Err(ServerError::InvalidReviewWorkspace);
            }
            return Ok(json!({"workspaceId":id}));
        }
        if bindings
            .keys()
            .filter(|(connection, _)| *connection == owner)
            .count()
            >= 64
        {
            return Err(ServerError::RequestLimit);
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(ServerError::MissingHome)?;
        let storage = home.join(".local/share/tandem/reviews/workspaces").join(id);
        bindings.insert(
            key,
            Binding {
                directory: directory.clone(),
                workspace: Arc::new(WorkspaceReview::new(directory, storage)),
            },
        );
        Ok(json!({"workspaceId":id}))
    }
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(ServerError::InvalidReviewWorkspace);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_ids_cannot_escape_private_storage() {
        for id in ["", "../other", "/absolute", "C:\\other", "a/b", "a\0b"] {
            assert!(validate_id(id).is_err());
        }
        assert!(validate_id("f5a2e6c7-24db-4247-a895-86fbc6bf261c").is_ok());
    }

    #[tokio::test]
    async fn unbound_connections_cannot_inspect_workspaces() {
        let host = ReviewHost::default();
        let result = host
            .dispatch(
                3,
                "get_workspace_review",
                &json!({"workspaceId":"another","comparison":"net"}),
            )
            .await;
        assert!(matches!(result, Err(ServerError::ReviewNotBound)));
    }

    #[tokio::test]
    async fn workspace_binding_cannot_be_remapped_or_shared() -> anyhow::Result<()> {
        let host = ReviewHost::default();
        let directory = std::fs::canonicalize(std::env::current_dir()?)?;
        let other = directory
            .parent()
            .ok_or_else(|| anyhow::anyhow!("fixture has no parent"))?
            .to_path_buf();
        host.bindings.lock().await.insert(
            (7, "workspace".into()),
            Binding {
                directory: directory.clone(),
                workspace: Arc::new(WorkspaceReview::new(
                    directory.clone(),
                    std::env::temp_dir(),
                )),
            },
        );
        host.dispatch(
            7,
            "open_review_workspace",
            &json!({"workspaceId":"workspace","directory":directory}),
        )
        .await?;
        let remapped = host
            .dispatch(
                7,
                "open_review_workspace",
                &json!({"workspaceId":"workspace","directory":other}),
            )
            .await;
        assert!(matches!(remapped, Err(ServerError::InvalidReviewWorkspace)));
        let foreign = host
            .dispatch(
                8,
                "get_workspace_review",
                &json!({"workspaceId":"workspace","comparison":"net"}),
            )
            .await;
        assert!(matches!(foreign, Err(ServerError::ReviewNotBound)));
        host.disconnect(7).await;
        assert!(host.bindings.lock().await.is_empty());
        Ok(())
    }
}
