//! Remote project discovery and JSON RPC over authenticated OpenSSH channels.
//!
//! The user's SSH configuration, agent, and known-host database remain the source
//! of trust. Tandem never supplies passwords or accepts an unknown host key.

mod bootstrap;
mod ssh;
mod transport;

pub use bootstrap::{
    ArtifactLocator, BootstrapReport, RemoteClient, RemoteDirectory, RemotePlatform,
};
pub use ssh::SshConnection;
pub use tandem_protocol::RpcErrorCode;
pub use transport::RemoteTransport;

/// Invalid destination fields rejected before OpenSSH is launched.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConnectionValidationError {
    #[error("host must be an SSH alias, hostname, or unbracketed IP address")]
    Host,
    #[error("invalid SSH username")]
    Username,
    #[error("port must be greater than zero")]
    Port,
    #[error("invalid identity file path")]
    IdentityFile,
}

/// Invalid directory input rejected before composing a remote shell command.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DirectoryValidationError {
    #[error("use a non-empty directory path")]
    Empty,
    #[error("directory path must be at most {limit} bytes long")]
    TooLong { limit: usize },
    #[error("directory path must not contain NUL bytes")]
    NullByte,
}

/// Invalid bootstrap responses are distinct from newline-delimited RPC errors.
#[derive(Debug, thiserror::Error)]
pub enum RemoteResponseError {
    #[error("environment probe was not UTF-8")]
    ProbeEncoding(#[source] std::str::Utf8Error),
    #[error("incomplete environment probe; expected OS, architecture, and version fields")]
    IncompleteProbe,
    #[error("expected one canonical directory, received {actual}")]
    CanonicalDirectoryCount { actual: usize },
    #[error("incomplete directory response")]
    IncompleteDirectory,
    #[error("directory names must be valid UTF-8")]
    DirectoryEncoding(#[source] std::str::Utf8Error),
    #[error("remote directory was not absolute")]
    RelativeDirectory,
}

/// Sanitized SSH diagnostics never contain arbitrary remote stderr.
#[derive(Clone, Copy, Debug, thiserror::Error)]
pub enum SshDiagnostic {
    #[error(
        "The host key is unknown or changed. Verify the host fingerprint using your normal SSH client, then reconnect."
    )]
    HostKey,
    #[error(
        "SSH authentication or remote permissions were denied. Check your username, identity file, and ssh-agent keys."
    )]
    PermissionDenied,
    #[error("The SSH hostname could not be resolved. Check the host or SSH config alias.")]
    Hostname,
    #[error("The host refused the SSH connection. Check its SSH service and port.")]
    ConnectionRefused,
    #[error("The SSH host is unreachable. Check its address, VPN, and network connection.")]
    Unreachable,
    #[error(
        "A remote directory or executable is missing. Check the selected path and remote runtime installation."
    )]
    MissingPath,
    #[error(
        "The remote executable does not match the host architecture. Install the correct tandem-server artifact."
    )]
    Architecture,
    #[error("Check SSH access, the selected directory, and the remote runtime installation.")]
    Other,
}

/// Shared disconnect causes preserve the original failure for every pending request.
#[derive(Debug, thiserror::Error)]
pub enum DisconnectReason {
    #[error("SSH exited with {status}. {diagnostic}")]
    ProcessExited {
        status: std::process::ExitStatus,
        diagnostic: SshDiagnostic,
    },
    #[error("The SSH process could not be monitored. {diagnostic}")]
    ProcessMonitor {
        #[source]
        source: std::io::Error,
        diagnostic: SshDiagnostic,
    },
    #[error("The remote response stream failed: {source}. {diagnostic}")]
    ResponseFailed {
        #[source]
        source: RemoteError,
        diagnostic: SshDiagnostic,
    },
    #[error("The remote server closed its output stream. {diagnostic}")]
    OutputClosed { diagnostic: SshDiagnostic },
    #[error("The remote response reader ended unexpectedly. {diagnostic}")]
    ReaderStopped {
        #[source]
        source: tokio::task::JoinError,
        diagnostic: SshDiagnostic,
    },
    #[error("The connection was closed")]
    Closed,
    #[error("The SSH input stream has closed")]
    InputClosed,
    #[error("The remote response channel ended")]
    ResponseChannelClosed,
}

/// Failures that can be handled without inspecting message text or credentials.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("Invalid SSH connection: {0}")]
    InvalidConnection(#[from] ConnectionValidationError),
    #[error("SSH {operation} failed ({status}). {detail}")]
    ConnectionFailed {
        operation: &'static str,
        status: std::process::ExitStatus,
        detail: SshDiagnostic,
    },
    #[error("SSH {operation} timed out after {seconds} seconds")]
    Timeout {
        operation: &'static str,
        seconds: u64,
    },
    #[error("Remote platform {os}/{architecture} is unsupported; use a Linux or macOS SSH host")]
    UnsupportedPlatform { os: String, architecture: String },
    #[error(
        "No tandem-server artifact for {target}. Place it at <TANDEM_REMOTE_ARTIFACTS>/{target}/tandem-server, or install the matching server on the host"
    )]
    ArtifactMissing { target: String },
    #[error(
        "Invalid tandem-server artifact {}: expected a non-empty executable smaller than 512 MiB", path.display()
    )]
    InvalidArtifact { path: std::path::PathBuf },
    #[error("Invalid remote response: {0}")]
    InvalidResponse(#[from] RemoteResponseError),
    #[error(
        "The remote server has an incompatible protocol or release. Reconnect the project to deploy the matching server."
    )]
    ServerVersionMismatch,
    #[error("Invalid remote RPC frame: {0}")]
    Protocol(#[from] tandem_protocol::ProtocolError),
    #[error(transparent)]
    Remote(#[from] tandem_protocol::RpcError),
    #[error("Remote connection closed. {0}")]
    Disconnected(#[source] std::sync::Arc<DisconnectReason>),
    #[error("Invalid remote directory: {0}")]
    InvalidDirectory(#[from] DirectoryValidationError),
    #[error("Remote output exceeded the {limit} byte size limit")]
    OutputTooLarge { limit: usize },
    #[error("SSH standard streams were not created")]
    MissingPipe,
    #[error("Remote request identifiers exhausted; reconnect")]
    RequestIdExhausted,
    #[error("Too many outstanding remote requests; wait for an earlier request to complete")]
    TooManyRequests,
    #[error("SSH I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

impl RemoteError {
    /// Classify failures without parsing their human-readable descriptions.
    #[must_use]
    pub const fn code(&self) -> RpcErrorCode {
        match self {
            Self::InvalidConnection(_)
            | Self::InvalidDirectory(_)
            | Self::Protocol(
                tandem_protocol::ProtocolError::InvalidCommand
                | tandem_protocol::ProtocolError::FrameTooLarge { .. },
            ) => RpcErrorCode::InvalidPayload,
            Self::Protocol(tandem_protocol::ProtocolError::Io(_)) => {
                RpcErrorCode::RuntimeUnavailable
            }
            Self::Timeout { .. } => RpcErrorCode::RuntimeTimeout,
            Self::TooManyRequests => RpcErrorCode::RuntimeBusy,
            Self::Remote(error) => error.code,
            Self::Protocol(_) | Self::MissingPipe | Self::RequestIdExhausted => {
                RpcErrorCode::Internal
            }
            Self::ConnectionFailed { .. }
            | Self::UnsupportedPlatform { .. }
            | Self::ArtifactMissing { .. }
            | Self::InvalidArtifact { .. }
            | Self::InvalidResponse(_)
            | Self::ServerVersionMismatch
            | Self::Disconnected(_)
            | Self::OutputTooLarge { .. }
            | Self::Io(_) => RpcErrorCode::RuntimeUnavailable,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn protocol_causes_and_remote_codes_remain_typed() {
        let error = RemoteError::from(tandem_protocol::ProtocolError::TruncatedFrame);
        assert_eq!(error.code(), RpcErrorCode::Internal);
        assert!(
            error
                .source()
                .and_then(|source| source.downcast_ref::<tandem_protocol::ProtocolError>())
                .is_some_and(|error| matches!(
                    error,
                    tandem_protocol::ProtocolError::TruncatedFrame
                ))
        );
        let error = RemoteError::from(tandem_protocol::RpcError {
            code: RpcErrorCode::ApprovalNotFound,
            message: "approval expired".into(),
        });
        assert_eq!(error.code(), RpcErrorCode::ApprovalNotFound);
        assert_eq!(
            RemoteError::from(DirectoryValidationError::Empty).code(),
            RpcErrorCode::InvalidPayload
        );
        assert_eq!(
            RemoteError::TooManyRequests.code(),
            RpcErrorCode::RuntimeBusy
        );
    }
}
