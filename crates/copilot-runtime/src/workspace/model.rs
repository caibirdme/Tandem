use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceComparison {
    Net,
    Staged,
    Unstaged,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewSelection {
    Workspace {
        comparison: WorkspaceComparison,
    },
    Session,
    Turn {
        #[serde(rename = "messageId")]
        message_id: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub id: String,
    pub captured_at: u64,
    pub complete: bool,
    pub excluded_files: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSummary {
    pub review_id: String,
    pub revision: String,
    pub workspace_path: String,
    pub repository_path: String,
    pub selection: ReviewSelection,
    pub captured_at: u64,
    pub files_total: usize,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub complete: bool,
    pub excluded_files: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChanged,
    Conflict,
    Unchanged,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Text,
    Binary,
    TooLarge,
    Unsupported,
    Conflict,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFile {
    pub id: String,
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub index_status: Option<String>,
    pub worktree_status: Option<String>,
    pub content_kind: ContentKind,
    pub old_mode: Option<String>,
    pub new_mode: Option<String>,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFilePage {
    pub files: Vec<ReviewFile>,
    pub next_cursor: Option<usize>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    Context,
    Addition,
    Deletion,
    Note,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_line: Option<u64>,
    pub new_line: Option<u64>,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    pub old_start: u64,
    pub new_start: u64,
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFileDiff {
    pub review_id: String,
    pub revision: String,
    pub file: ReviewFile,
    pub patch: Option<String>,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewErrorCode {
    Unavailable,
    NotRepository,
    GitUnavailable,
    NotFound,
    Expired,
    InvalidInput,
    Limit,
    Timeout,
    Internal,
}

#[derive(Debug, thiserror::Error)]
pub enum ReviewError {
    #[error("Install Git on the execution host to review changes")]
    GitUnavailable(#[source] std::io::Error),
    #[error("Git {version} is unsupported; install Git 2.45 or newer on the execution host")]
    GitVersionUnsupported { version: String },
    #[error("The selected directory is not inside a Git worktree")]
    NotRepository,
    #[error("The project directory is unavailable")]
    WorkspaceUnavailable(#[source] std::io::Error),
    #[error("Review storage could not be accessed")]
    Storage(#[source] std::io::Error),
    #[error("Review data could not be encoded or decoded")]
    Serialization(#[from] serde_json::Error),
    #[error("Git could not complete {operation} (exit code {code:?})")]
    GitFailed {
        operation: &'static str,
        code: Option<i32>,
    },
    #[error("Git inspection could not be started")]
    GitIo(#[source] std::io::Error),
    #[error("Git inspection exceeded its time limit; refresh and try again")]
    Timeout,
    #[error("Review data exceeded its size limit")]
    Limit,
    #[error("This review or snapshot is no longer available")]
    NotFound,
    #[error("This review revision has expired; refresh the file list")]
    RevisionExpired,
    #[error("The review identifier or file path is invalid")]
    InvalidInput,
    #[error("The workspace changed while it was being captured; refresh and try again")]
    WorkspaceChanged,
    #[error("Git returned an unsupported inspection record")]
    InvalidGitOutput,
    #[error(
        "Workspace review is unavailable while Git content filters are assigned; session snapshots remain available"
    )]
    ContentFilters,
    #[error("A review worker could not finish")]
    Worker(#[source] tokio::task::JoinError),
    #[error("The workspace inspection coordinator is unavailable; restart the application")]
    CoordinatorUnavailable,
}

impl ReviewError {
    #[must_use]
    pub const fn code(&self) -> ReviewErrorCode {
        match self {
            Self::GitUnavailable(_) | Self::GitVersionUnsupported { .. } => {
                ReviewErrorCode::GitUnavailable
            }
            Self::NotRepository => ReviewErrorCode::NotRepository,
            Self::WorkspaceUnavailable(_)
            | Self::GitFailed { .. }
            | Self::WorkspaceChanged
            | Self::ContentFilters => ReviewErrorCode::Unavailable,
            Self::NotFound => ReviewErrorCode::NotFound,
            Self::RevisionExpired => ReviewErrorCode::Expired,
            Self::InvalidInput => ReviewErrorCode::InvalidInput,
            Self::Limit => ReviewErrorCode::Limit,
            Self::Timeout => ReviewErrorCode::Timeout,
            Self::Storage(_)
            | Self::Serialization(_)
            | Self::GitIo(_)
            | Self::InvalidGitOutput
            | Self::CoordinatorUnavailable
            | Self::Worker(_) => ReviewErrorCode::Internal,
        }
    }
}
