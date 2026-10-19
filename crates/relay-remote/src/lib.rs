//! Remote project discovery and JSON RPC over authenticated OpenSSH channels.
//!
//! The user's SSH configuration, agent, and known-host database remain the source
//! of trust. Relay never supplies passwords or accepts an unknown host key.

mod bootstrap;
mod ssh;
mod transport;

pub use bootstrap::{
    ArtifactLocator, BootstrapReport, RemoteClient, RemoteDirectory, RemotePlatform,
};
pub use ssh::SshConnection;
pub use transport::RemoteTransport;

/// Failures that can be presented without exposing request payloads or credentials.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("Invalid SSH connection: {0}")]
    InvalidConnection(String),
    #[error("SSH {operation} failed ({status}). {detail}")]
    ConnectionFailed {
        operation: &'static str,
        status: String,
        detail: String,
    },
    #[error("SSH {operation} timed out after {seconds} seconds")]
    Timeout {
        operation: &'static str,
        seconds: u64,
    },
    #[error("Remote platform {os}/{architecture} is unsupported; use a Linux or macOS SSH host")]
    UnsupportedPlatform { os: String, architecture: String },
    #[error(
        "No relay-server artifact for {target}. Place it at <RELAY_REMOTE_ARTIFACTS>/{target}/relay-server, or install the matching server on the host"
    )]
    ArtifactMissing { target: String },
    #[error("Invalid relay-server artifact {path}: {reason}")]
    InvalidArtifact { path: String, reason: &'static str },
    #[error("Invalid remote response: {0}")]
    Protocol(String),
    #[error("Remote server error ({code}): {message}")]
    Remote { code: String, message: String },
    #[error("Remote connection closed. {0}")]
    Disconnected(String),
    #[error("Invalid remote directory: {0}")]
    InvalidDirectory(String),
    #[error("SSH I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

impl From<relay_protocol::ProtocolError> for RemoteError {
    fn from(error: relay_protocol::ProtocolError) -> Self {
        match error {
            relay_protocol::ProtocolError::InvalidFrame(message) => Self::Protocol(message),
            relay_protocol::ProtocolError::Io(error) => Self::Io(error),
        }
    }
}
