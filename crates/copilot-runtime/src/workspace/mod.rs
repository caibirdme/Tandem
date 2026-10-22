//! Read-only Git inspection and immutable raw file snapshots on the execution host.
//!
//! No operation starts an SDK session or changes the project's index, refs, or files.

mod capture;
mod diff;
mod git;
mod model;
mod status;
#[cfg(test)]
mod tests;

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{SystemTime, UNIX_EPOCH},
};

pub use model::{
    ContentKind, DiffHunk, DiffLine, DiffLineKind, FileStatus, ReviewError, ReviewErrorCode,
    ReviewFile, ReviewFileDiff, ReviewFilePage, ReviewSelection, ReviewSummary, Snapshot,
    WorkspaceComparison,
};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

type Result<T> = std::result::Result<T, ReviewError>;

const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_CAPTURE_BYTES: usize = 256 * 1024 * 1024;
const MAX_FILES: usize = 20_000;
const MAX_PATCH_BYTES: usize = 512 * 1024;
const MAX_METADATA_BYTES: usize = 32 * 1024 * 1024;
type ScanGate = tokio::sync::Mutex<()>;
static SCAN_GATES: OnceLock<Mutex<HashMap<PathBuf, Weak<ScanGate>>>> = OnceLock::new();

/// A project-scoped inspector; private storage may be shared by its sessions.
#[derive(Clone)]
pub struct WorkspaceReview {
    project: PathBuf,
    storage: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Entry {
    path: String,
    object: Option<String>,
    mode: String,
    kind: ContentKind,
    size: u64,
}

#[derive(Clone, Deserialize, Serialize)]
struct Manifest {
    snapshot: Snapshot,
    project: String,
    repository: String,
    entries: BTreeMap<String, Entry>,
}

#[derive(Deserialize, Serialize)]
struct SavedReview {
    identity: String,
    summary: ReviewSummary,
    files: Vec<SavedFile>,
}

#[derive(Clone, Deserialize, Serialize)]
struct SavedFile {
    metadata: ReviewFile,
    before: Option<Entry>,
    after: Option<Entry>,
}

impl WorkspaceReview {
    /// Configure project-scoped inspection without starting an agent.
    #[must_use]
    pub fn new(project: PathBuf, storage: PathBuf) -> Self {
        let project = std::fs::canonicalize(&project).unwrap_or(project);
        Self { project, storage }
    }

    async fn initialize_storage(&self) -> Result<()> {
        tokio::fs::create_dir_all(&self.storage)
            .await
            .map_err(ReviewError::Storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&self.storage, std::fs::Permissions::from_mode(0o700))
                .await
                .map_err(ReviewError::Storage)?;
        }
        Ok(())
    }

    async fn scan_gate(&self) -> Result<tokio::sync::OwnedMutexGuard<()>> {
        let (_, root) = self.roots().await?;
        let gate = {
            let mut gates = SCAN_GATES
                .get_or_init(Mutex::default)
                .lock()
                .map_err(|_| ReviewError::CoordinatorUnavailable)?;
            gates.retain(|_, gate| gate.strong_count() > 0);
            if let Some(gate) = gates.get(&root).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(ScanGate::new(()));
                gates.insert(root, Arc::downgrade(&gate));
                gate
            }
        };
        Ok(gate.lock_owned().await)
    }

    /// Read an immutable review summary.
    ///
    /// # Errors
    /// Returns a typed failure for unavailable data, invalid identifiers, or a stale revision.
    pub async fn summary(&self, review_id: &str, revision: &str) -> Result<ReviewSummary> {
        Ok(self.review(review_id, revision).await?.summary)
    }

    /// Read one bounded page of file metadata.
    ///
    /// # Errors
    /// Returns a typed failure for unavailable data, invalid identifiers, or a stale revision.
    pub async fn files(
        &self,
        review_id: &str,
        revision: &str,
        cursor: usize,
    ) -> Result<ReviewFilePage> {
        let review = self.review(review_id, revision).await?;
        if cursor > review.files.len() {
            return Err(ReviewError::InvalidInput);
        }
        let mut end = cursor;
        let mut bytes = 128usize;
        for file in review.files.iter().skip(cursor).take(200) {
            let size = serde_json::to_vec(&file.metadata)?.len();
            if bytes.saturating_add(size) > 256 * 1024 {
                break;
            }
            bytes += size;
            end += 1;
        }
        if end == cursor && cursor < review.files.len() {
            return Err(ReviewError::Limit);
        }
        Ok(ReviewFilePage {
            files: review.files[cursor..end]
                .iter()
                .map(|file| file.metadata.clone())
                .collect(),
            next_cursor: (end < review.files.len()).then_some(end),
        })
    }

    /// Read a file from the same immutable revision as its list entry.
    ///
    /// # Errors
    /// Returns a typed failure for unavailable data, invalid identifiers, or a stale revision.
    pub async fn file(
        &self,
        review_id: &str,
        revision: &str,
        file_id: &str,
    ) -> Result<ReviewFileDiff> {
        let review = self.review(review_id, revision).await?;
        validate_id(file_id)?;
        let file = review
            .files
            .into_iter()
            .find(|file| file.metadata.id == file_id)
            .ok_or(ReviewError::NotFound)?;
        let path = self
            .storage
            .join("reviews")
            .join(review_id)
            .join(format!("{file_id}.json"));
        match read_json_limit(&path, MAX_PATCH_BYTES).await {
            Ok(detail) => Ok(detail),
            Err(ReviewError::NotFound) => {
                self.build_file_detail(review_id.to_owned(), file.metadata, file.before, file.after)
                    .await
            }
            Err(error) => Err(error),
        }
    }

    async fn review(&self, review_id: &str, revision: &str) -> Result<SavedReview> {
        validate_id(review_id)?;
        let review: SavedReview = read_json(
            &self
                .storage
                .join("reviews")
                .join(format!("{review_id}.json")),
        )
        .await?;
        if review.summary.revision != revision {
            return Err(ReviewError::RevisionExpired);
        }
        if !self.matches_project(&review.summary.workspace_path) {
            return Err(ReviewError::InvalidInput);
        }
        Ok(review)
    }

    async fn manifest(&self, id: &str) -> Result<Manifest> {
        validate_id(id)?;
        let manifest: Manifest =
            read_json(&self.storage.join("snapshots").join(format!("{id}.json"))).await?;
        if !self.matches_project(&manifest.project) {
            return Err(ReviewError::InvalidInput);
        }
        Ok(manifest)
    }

    fn matches_project(&self, recorded: &str) -> bool {
        let project = self.project.to_string_lossy();
        #[cfg(windows)]
        {
            let normalize = |path: &str| {
                path.replace('\\', "/")
                    .trim_start_matches("//?/")
                    .to_lowercase()
            };
            normalize(&project) == normalize(recorded)
        }
        #[cfg(not(windows))]
        {
            Path::new(project.as_ref()) == Path::new(recorded)
        }
    }
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 100
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(ReviewError::InvalidInput);
    }
    Ok(())
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

async fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    read_json_limit(path, MAX_METADATA_BYTES).await
}

async fn read_json_limit<T: serde::de::DeserializeOwned>(path: &Path, limit: usize) -> Result<T> {
    let bytes = read_bytes_limit(path, limit).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn read_bytes_limit(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let file = tokio::fs::File::open(path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ReviewError::NotFound
        } else {
            ReviewError::Storage(error)
        }
    })?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(limit + 1).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .await
        .map_err(ReviewError::Storage)?;
    if bytes.len() > limit {
        return Err(ReviewError::Limit);
    }
    Ok(bytes)
}

async fn save_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().ok_or(ReviewError::InvalidInput)?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(ReviewError::Storage)?;
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_METADATA_BYTES {
        return Err(ReviewError::Limit);
    }
    let temporary = tempfile::NamedTempFile::new_in(parent).map_err(ReviewError::Storage)?;
    tokio::fs::write(temporary.path(), bytes)
        .await
        .map_err(ReviewError::Storage)?;
    match temporary.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(ReviewError::Storage(error.error)),
    }
}
