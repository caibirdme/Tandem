use std::path::PathBuf;

use copilot_runtime::Error as RuntimeError;
use tandem_protocol::{RpcError, RpcErrorCode};

/// Failures retain their origin until the stdio response boundary.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ServerError {
    #[error("Unknown option: {option}; use --help for remote server options")]
    UnknownOption { option: String },
    #[error("Choose only one tandem-server mode")]
    ConflictingModes,
    #[error("Unsupported remote command")]
    UnsupportedCommand,
    #[error("{field} is required and must be a string")]
    RequiredField { field: &'static str },
    #[error("{field} must be a boolean")]
    InvalidBoolean { field: &'static str },
    #[error("Invalid {field} payload")]
    InvalidPayload {
        field: &'static str,
        #[source]
        source: tandem_protocol::JsonDecodeError,
    },
    #[error("Session not connected; start or resume the session before sending a command")]
    SessionNotConnected,
    #[error("The prepared message is missing or has expired. Prepare the message again.")]
    PreparedMessageUnavailable,
    #[error("HOME is required on a remote host")]
    MissingHome,
    #[error("Runtime unavailable; start the remote runtime again")]
    RuntimeUnavailable,
    #[error("SDK state path is not a directory: {}", path.display())]
    StatePathNotDirectory { path: PathBuf },
    #[error("Cannot inspect SDK state directory {}: {source}", path.display())]
    StateDirectoryIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "The remote event stream overflowed and dropped {dropped} events. Send the message again to resume the session."
    )]
    EventStreamOverflow { dropped: u64 },
    #[error("Remote output closed before the response could be delivered")]
    OutputClosed,
    #[error("Remote output did not drain before shutdown")]
    OutputDrainTimeout(#[source] tokio::time::error::Elapsed),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error(transparent)]
    Protocol(#[from] tandem_protocol::ProtocolError),
    #[error("Remote stdio failed: {0}")]
    Io(#[from] std::io::Error),
}

impl ServerError {
    const fn code(&self) -> RpcErrorCode {
        match self {
            Self::UnsupportedCommand => RpcErrorCode::UnsupportedCommand,
            Self::Runtime(RuntimeError::Skills(error)) => match error {
                copilot_runtime::SkillError::PreparationTimeout
                | copilot_runtime::SkillError::DiscoveryTimeout => RpcErrorCode::RuntimeTimeout,
                _ => RpcErrorCode::InvalidPayload,
            },
            Self::RequiredField { .. }
            | Self::InvalidBoolean { .. }
            | Self::InvalidPayload { .. }
            | Self::PreparedMessageUnavailable
            | Self::Runtime(RuntimeError::Attachments(_) | RuntimeError::InvalidConfiguration(_)) => {
                RpcErrorCode::InvalidPayload
            }
            Self::SessionNotConnected | Self::Runtime(RuntimeError::Disconnected) => {
                RpcErrorCode::SessionNotConnected
            }
            Self::RuntimeUnavailable | Self::MissingHome => RpcErrorCode::RuntimeUnavailable,
            Self::StatePathNotDirectory { .. } | Self::StateDirectoryIo { .. } => {
                RpcErrorCode::StateDirectoryUnavailable
            }
            Self::Runtime(RuntimeError::Busy) => RpcErrorCode::RuntimeBusy,
            Self::Runtime(RuntimeError::StartupTimeout | RuntimeError::AbortTimeout) => {
                RpcErrorCode::RuntimeTimeout
            }
            Self::Runtime(RuntimeError::UnknownApproval) => RpcErrorCode::ApprovalNotFound,
            Self::Runtime(_) | Self::EventStreamOverflow { .. } => RpcErrorCode::RuntimeFailure,
            Self::UnknownOption { .. }
            | Self::ConflictingModes
            | Self::OutputClosed
            | Self::OutputDrainTimeout(_)
            | Self::Protocol(_)
            | Self::Io(_) => RpcErrorCode::Internal,
        }
    }

    pub(crate) fn into_rpc(self) -> RpcError {
        RpcError {
            code: self.code(),
            message: self.to_string(),
        }
    }
}

pub(crate) type Result<T> = std::result::Result<T, ServerError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_codes_preserve_runtime_failure_types() {
        for (error, expected) in [
            (
                ServerError::UnsupportedCommand,
                RpcErrorCode::UnsupportedCommand,
            ),
            (
                ServerError::RequiredField { field: "content" },
                RpcErrorCode::InvalidPayload,
            ),
            (
                ServerError::SessionNotConnected,
                RpcErrorCode::SessionNotConnected,
            ),
            (ServerError::MissingHome, RpcErrorCode::RuntimeUnavailable),
            (
                ServerError::Runtime(RuntimeError::Busy),
                RpcErrorCode::RuntimeBusy,
            ),
            (
                ServerError::Runtime(RuntimeError::StartupTimeout),
                RpcErrorCode::RuntimeTimeout,
            ),
            (
                ServerError::Runtime(RuntimeError::AbortTimeout),
                RpcErrorCode::RuntimeTimeout,
            ),
            (
                ServerError::Runtime(RuntimeError::Skills(
                    copilot_runtime::SkillError::DiscoveryTimeout,
                )),
                RpcErrorCode::RuntimeTimeout,
            ),
            (
                ServerError::Runtime(RuntimeError::UnknownApproval),
                RpcErrorCode::ApprovalNotFound,
            ),
            (
                ServerError::Runtime(RuntimeError::Disconnected),
                RpcErrorCode::SessionNotConnected,
            ),
            (
                ServerError::Runtime(RuntimeError::InvalidConfiguration(
                    copilot_runtime::ConfigurationError::EmptyPrompt,
                )),
                RpcErrorCode::InvalidPayload,
            ),
            (
                ServerError::Runtime(RuntimeError::Attachments(
                    copilot_runtime::AttachmentError::TooMany { limit: 10 },
                )),
                RpcErrorCode::InvalidPayload,
            ),
        ] {
            assert_eq!(error.into_rpc().code, expected);
        }
    }

    #[test]
    fn payload_failures_preserve_safe_parse_context_without_echoing_input() {
        let Err(source) = serde_json::from_str::<bool>("\"private-key\"") else {
            panic!("a string must not decode as a boolean");
        };
        let error = ServerError::InvalidPayload {
            field: "options",
            source: source.into(),
        };
        assert!(!format!("{error:?}").contains("private-key"));
        assert!(
            std::error::Error::source(&error)
                .and_then(|source| source.downcast_ref::<tandem_protocol::JsonDecodeError>())
                .is_some()
        );
        assert_eq!(error.into_rpc().code, RpcErrorCode::InvalidPayload);
    }
}
