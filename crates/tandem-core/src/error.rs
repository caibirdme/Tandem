use std::path::PathBuf;

use serde::Serialize;

use crate::{SecretError, StorageError, model::ValidationError};

/// Application failures retain their domain and original cause until the transport boundary.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Select 7, 14, or 30 consecutive local calendar days.")]
    InvalidUsageRange,
    #[error(transparent)]
    ModelDiscovery(#[from] crate::ModelDiscoveryError),
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Secret(#[from] SecretError),
    #[error(transparent)]
    Runtime(#[from] copilot_runtime::Error),
    #[error(transparent)]
    Review(#[from] copilot_runtime::workspace::ReviewError),
    #[error("The review operation stopped unexpectedly. Refresh the changes to try again.")]
    ReviewTask(#[source] tokio::task::JoinError),
    #[error(
        "No captured baseline is available for this review. Run a new turn to record its changes."
    )]
    ReviewBaselineUnavailable,
    #[error("This review does not belong to the selected conversation.")]
    ReviewNotRegistered,
    #[error("The review revision changed. Refresh the file list and try again.")]
    ReviewRevisionExpired,
    #[error("The review response has an invalid format.")]
    InvalidReviewData(#[source] EventDecodeError),
    #[error(transparent)]
    Attachment(#[from] copilot_runtime::AttachmentError),
    #[error("This conversation contains an attachment the selected provider cannot send: {source}")]
    ConversationAttachmentProtocol {
        session_id: String,
        #[source]
        source: copilot_runtime::AttachmentError,
    },
    #[error(transparent)]
    Remote(#[from] tandem_remote::RemoteError),
    #[error("Filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Application data could not be encoded or decoded: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Cannot determine the application data directory")]
    DataDirectoryUnavailable,
    #[error("Cannot determine the user's home directory")]
    HomeDirectoryUnavailable,
    #[error("The Copilot runtime failed to start: {0}")]
    RuntimeStartup(#[source] copilot_runtime::Error),
    #[error("The specified Copilot CLI file does not exist: {}", path.display())]
    CliNotFound { path: PathBuf },
    #[error("Stop running conversations before continuing.")]
    ConversationsRunning,
    #[error("The directory does not exist or cannot be accessed: {0}")]
    DirectoryUnavailable(#[source] std::io::Error),
    #[error("The project path must be a directory.")]
    ProjectPathNotDirectory,
    #[error("This project does not have an SSH connection.")]
    SshConnectionRequired,
    #[error("Stop the project's running work before continuing.")]
    ProjectRunning,
    #[error("The provider ID is invalid")]
    InvalidProviderId(#[source] uuid::Error),
    #[error(
        "Deleting the API key failed ({secret}), and the provider record could not be restored: {storage}"
    )]
    ProviderRestore {
        secret: Box<SecretError>,
        #[source]
        storage: Box<StorageError>,
    },
    #[error(
        "Saving provider settings failed ({storage}), and the previous API key could not be restored: {secret}"
    )]
    ProviderSaveRestore {
        #[source]
        storage: Box<StorageError>,
        secret: Box<SecretError>,
    },
    #[error(
        "Saving telemetry settings failed ({storage}), and the previous API key could not be restored: {secret}"
    )]
    TelemetryPreferencesRestore {
        #[source]
        storage: Box<StorageError>,
        secret: Box<SecretError>,
    },
    #[error("Wait for this provider's conversations to finish before changing its settings.")]
    ProviderRunning,
    #[error("The selected model is not configured for this provider. Select a model again.")]
    ModelNotConfigured {
        provider_id: String,
        model_id: String,
    },
    #[error("Chat does not support automatic permission approval. Create project work instead.")]
    ChatPermissionMode,
    #[error("Only project work supports switching permission modes.")]
    WorkSessionRequired,
    #[error(
        "Work is initializing or processing an operation. Try switching permission modes again shortly."
    )]
    PermissionModeChangeBusy,
    #[error("Wait for the work to finish or stop it before switching permission modes.")]
    PermissionModeChangeRunning,
    #[error(
        "The conversation is initializing or processing an operation. Try switching models again shortly."
    )]
    ModelChangeBusy,
    #[error("Wait for the conversation to finish or stop it before switching models.")]
    ModelChangeRunning,
    #[error(
        "The model was saved, but the previous session connection could not be cleaned up. Try sending again."
    )]
    ModelConnectionCleanup(#[source] tokio::task::JoinError),
    #[error("Add a message or attachment, and keep message text within 500 KB.")]
    InvalidMessage,
    #[error("The conversation is running. Wait for it to finish or click Stop.")]
    ConversationRunning,
    #[error("The session connection is no longer valid. Try again.")]
    SessionDisconnected,
    #[error("Sending was canceled before the message was sent. Try again.")]
    SubmissionCanceled,
    #[error("This message is already recorded. Review the conversation before sending again.")]
    MessageAlreadyRecorded,
    #[error("The session was disconnected, and this permission request is no longer valid.")]
    PermissionSessionDisconnected,
    #[error("The runtime event stream was interrupted: {0}")]
    RuntimeEventStream(#[source] tokio::sync::broadcast::error::RecvError),
    #[error("The SSH event stream was interrupted: {0}")]
    RemoteEventStream(#[source] tokio::sync::broadcast::error::RecvError),
    #[error("The remote event format is invalid: {0}")]
    InvalidRemoteEvent(#[source] EventDecodeError),
    #[error(
        "The remote connection delivered an event for another session. Reconnect to resume this conversation."
    )]
    RemoteSessionMismatch,
    #[error("The remote session did not disconnect in time. Its SSH connection has been closed.")]
    RemoteDisconnectTimeout(#[source] tokio::time::error::Elapsed),
    #[error("The runtime returned an invalid session state: {0}")]
    InvalidRuntimeState(#[source] EventDecodeError),
    #[error("The remote skill catalog is invalid: {0}")]
    InvalidSkillCatalog(#[source] EventDecodeError),
    #[error("Skills can only be selected for project work.")]
    ChatSkillSelection,
    #[error("Multi-agent mode is only available for project work.")]
    ChatFleetMode,
    #[error("The prepared message belongs to a different session. Send the message again.")]
    PreparedTurnMismatch,
    #[error("Conversation event forwarding has stopped")]
    ForwardingStopped(#[source] tokio::sync::watch::error::RecvError),
    #[error("The conversation completion event is still being processed. Try again shortly.")]
    CompletionTimeout(#[source] tokio::time::error::Elapsed),
    #[error("Session state lock is unavailable. Restart the application.")]
    SessionStatePoisoned,
    #[error("Title task state is unavailable. Restart the application.")]
    TitleStatePoisoned,
    #[error("The application is shutting down.")]
    ShuttingDown,
    #[error("Runtime settings are being updated. Try generating the title again shortly.")]
    TitleConfigurationUpdating,
    #[error("Complete a conversation turn before generating a title.")]
    TitleExchangeRequired,
    #[error("Too many titles are being generated. Try again shortly.")]
    TitleQueueFull,
    #[error("Title generation timed out")]
    TitleTimeout(#[source] tokio::time::error::Elapsed),
    #[error("The title must be a single line of text.")]
    TitleNotSingleLine,
    #[error("The title format is invalid.")]
    InvalidTitleFormat,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Stable categories for transport policy; callers can match `Error` for finer detail.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidInput,
    NotFound,
    Conflict,
    Unavailable,
    Internal,
}

impl From<tandem_remote::RpcErrorCode> for ErrorCode {
    fn from(code: tandem_remote::RpcErrorCode) -> Self {
        match code {
            tandem_remote::RpcErrorCode::InvalidPayload
            | tandem_remote::RpcErrorCode::UnsupportedCommand
            | tandem_remote::RpcErrorCode::ReviewInvalidInput => Self::InvalidInput,
            tandem_remote::RpcErrorCode::ApprovalNotFound
            | tandem_remote::RpcErrorCode::ReviewNotFound => Self::NotFound,
            tandem_remote::RpcErrorCode::RuntimeBusy
            | tandem_remote::RpcErrorCode::ReviewExpired => Self::Conflict,
            tandem_remote::RpcErrorCode::Internal | tandem_remote::RpcErrorCode::ReviewFailure => {
                Self::Internal
            }
            tandem_remote::RpcErrorCode::SessionNotConnected
            | tandem_remote::RpcErrorCode::RuntimeUnavailable
            | tandem_remote::RpcErrorCode::RuntimeFailure
            | tandem_remote::RpcErrorCode::RuntimeTimeout
            | tandem_remote::RpcErrorCode::StateDirectoryUnavailable
            | tandem_remote::RpcErrorCode::ReviewUnavailable
            | tandem_remote::RpcErrorCode::ReviewNotRepository
            | tandem_remote::RpcErrorCode::ReviewGitUnavailable
            | tandem_remote::RpcErrorCode::ReviewLimit
            | tandem_remote::RpcErrorCode::ReviewTimeout => Self::Unavailable,
        }
    }
}

/// Serializable command failure shared by desktop IPC and development HTTP.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub code: ErrorCode,
    pub error: String,
}

impl From<Error> for ErrorResponse {
    fn from(error: Error) -> Self {
        Self {
            code: error.code(),
            error: error.to_string(),
        }
    }
}

impl Error {
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::DirectoryUnavailable(source) if source.kind() == std::io::ErrorKind::NotFound => {
                ErrorCode::NotFound
            }
            Self::ModelDiscovery(error) => error.code(),
            Self::Remote(error) => error.code().into(),
            Self::Runtime(error) => runtime_error_code(error),
            Self::Review(error) => review_error_code(error.code()),
            Self::Attachment(_)
            | Self::ConversationAttachmentProtocol { .. }
            | Self::Validation(_)
            | Self::InvalidProviderId(_)
            | Self::ProjectPathNotDirectory
            | Self::SshConnectionRequired
            | Self::InvalidUsageRange
            | Self::ChatPermissionMode
            | Self::WorkSessionRequired
            | Self::InvalidMessage
            | Self::ChatFleetMode
            | Self::ChatSkillSelection
            | Self::TitleNotSingleLine
            | Self::InvalidTitleFormat
            | Self::TitleExchangeRequired => ErrorCode::InvalidInput,
            Self::Storage(
                StorageError::ProviderNotFound { .. }
                | StorageError::ProjectNotFound { .. }
                | StorageError::SessionNotFound { .. }
                | StorageError::AttachmentNotFound { .. },
            )
            | Self::ModelNotConfigured { .. }
            | Self::ReviewNotRegistered
            | Self::CliNotFound { .. } => ErrorCode::NotFound,
            Self::Storage(
                StorageError::ProviderInUse { .. }
                | StorageError::ActivityConflict { .. }
                | StorageError::ReasoningConflict { .. }
                | StorageError::AgentConflict { .. }
                | StorageError::AssistantConflict { .. },
            )
            | Self::ConversationsRunning
            | Self::ProjectRunning
            | Self::ProviderRunning
            | Self::PermissionModeChangeBusy
            | Self::PermissionModeChangeRunning
            | Self::ModelChangeBusy
            | Self::ModelChangeRunning
            | Self::ConversationRunning
            | Self::MessageAlreadyRecorded
            | Self::ReviewRevisionExpired
            | Self::TitleConfigurationUpdating
            | Self::TitleQueueFull => ErrorCode::Conflict,
            Self::DataDirectoryUnavailable
            | Self::HomeDirectoryUnavailable
            | Self::DirectoryUnavailable(_)
            | Self::RuntimeStartup(_)
            | Self::SubmissionCanceled
            | Self::SessionDisconnected
            | Self::PermissionSessionDisconnected
            | Self::RuntimeEventStream(_)
            | Self::RemoteEventStream(_)
            | Self::RemoteSessionMismatch
            | Self::RemoteDisconnectTimeout(_)
            | Self::ForwardingStopped(_)
            | Self::CompletionTimeout(_)
            | Self::ReviewBaselineUnavailable
            | Self::ShuttingDown
            | Self::TitleTimeout(_) => ErrorCode::Unavailable,
            Self::Storage(
                StorageError::OpenDatabase { .. }
                | StorageError::Database(_)
                | StorageError::Json(_)
                | StorageError::AttachmentCountMismatch
                | StorageError::LockPoisoned,
            )
            | Self::Secret(
                SecretError::OpenDatabase { .. }
                | SecretError::Database(_)
                | SecretError::LockPoisoned,
            )
            | Self::Io(_)
            | Self::Json(_)
            | Self::ProviderRestore { .. }
            | Self::ProviderSaveRestore { .. }
            | Self::TelemetryPreferencesRestore { .. }
            | Self::ModelConnectionCleanup(_)
            | Self::InvalidRemoteEvent(_)
            | Self::InvalidRuntimeState(_)
            | Self::InvalidSkillCatalog(_)
            | Self::InvalidReviewData(_)
            | Self::ReviewTask(_)
            | Self::PreparedTurnMismatch
            | Self::SessionStatePoisoned
            | Self::TitleStatePoisoned => ErrorCode::Internal,
        }
    }
}

fn review_error_code(code: copilot_runtime::workspace::ReviewErrorCode) -> ErrorCode {
    use copilot_runtime::workspace::ReviewErrorCode;

    match code {
        ReviewErrorCode::InvalidInput => ErrorCode::InvalidInput,
        ReviewErrorCode::NotFound => ErrorCode::NotFound,
        ReviewErrorCode::Expired => ErrorCode::Conflict,
        ReviewErrorCode::Internal => ErrorCode::Internal,
        _ => ErrorCode::Unavailable,
    }
}

fn runtime_error_code(error: &copilot_runtime::Error) -> ErrorCode {
    use copilot_runtime::Error as RuntimeError;
    match error {
        RuntimeError::Skills(error) => match error {
            copilot_runtime::SkillError::PreparationTimeout
            | copilot_runtime::SkillError::DiscoveryTimeout => ErrorCode::Unavailable,
            _ => ErrorCode::InvalidInput,
        },
        RuntimeError::InvalidConfiguration(_) | RuntimeError::Attachments(_) => {
            ErrorCode::InvalidInput
        }
        RuntimeError::UnknownApproval => ErrorCode::NotFound,
        RuntimeError::Telemetry(error) => match error {
            copilot_runtime::TelemetryError::Grpc(copilot_runtime::GrpcError::InvalidRecord {
                ..
            })
            | copilot_runtime::TelemetryError::Storage(_)
            | copilot_runtime::TelemetryError::Client(_)
            | copilot_runtime::TelemetryError::InvalidRecord(_)
            | copilot_runtime::TelemetryError::RecordLimit
            | copilot_runtime::TelemetryError::QueueLimit
            | copilot_runtime::TelemetryError::InvalidSpan
            | copilot_runtime::TelemetryError::TaskStopped => ErrorCode::Internal,
            copilot_runtime::TelemetryError::Grpc(error) => {
                if error.is_invalid_input() {
                    ErrorCode::InvalidInput
                } else {
                    ErrorCode::Unavailable
                }
            }
            copilot_runtime::TelemetryError::HttpStatus { status: 408 | 429 } => {
                ErrorCode::Unavailable
            }
            copilot_runtime::TelemetryError::HttpStatus { status: 400..=499 }
            | copilot_runtime::TelemetryError::RejectedSpans { .. } => ErrorCode::InvalidInput,
            copilot_runtime::TelemetryError::Transport(_)
            | copilot_runtime::TelemetryError::HttpStatus { .. }
            | copilot_runtime::TelemetryError::InvalidResponse
            | copilot_runtime::TelemetryError::IncompleteTrace { .. }
            | copilot_runtime::TelemetryError::SparseFile(_)
            | copilot_runtime::TelemetryError::SparseFileSetup
            | copilot_runtime::TelemetryError::FlushTimeout => ErrorCode::Unavailable,
        },
        RuntimeError::Busy => ErrorCode::Conflict,
        RuntimeError::Serialization(_) => ErrorCode::Internal,
        RuntimeError::Sdk(_)
        | RuntimeError::Io(_)
        | RuntimeError::Disconnected
        | RuntimeError::FleetToolsRejected
        | RuntimeError::FleetNotStarted
        | RuntimeError::StartupTimeout
        | RuntimeError::AbortTimeout
        | RuntimeError::Shutdown(_)
        | RuntimeError::Summary(_) => ErrorCode::Unavailable,
    }
}

/// Event parse errors retain category and location without echoing private event values.
#[derive(Debug, thiserror::Error)]
#[error("{category:?} error at line {line}, column {column}")]
pub struct EventDecodeError {
    pub category: serde_json::error::Category,
    pub line: usize,
    pub column: usize,
}

impl From<serde_json::Error> for EventDecodeError {
    fn from(error: serde_json::Error) -> Self {
        Self {
            category: error.classify(),
            line: error.line(),
            column: error.column(),
        }
    }
}
