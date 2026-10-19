//! Typed runtime failures and validation outcomes.

/// Invalid application settings, without retaining provider credentials or URLs.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConfigurationError {
    #[error("context window must be positive")]
    NonPositiveContextWindow,
    #[error("maximum output tokens must be positive")]
    NonPositiveMaxOutputTokens,
    #[error("maximum output tokens must be smaller than the context window")]
    OutputExceedsContextWindow,
    #[error("model ID cannot be empty")]
    EmptyModelId,
    #[error("endpoint must be an absolute HTTP(S) URL")]
    InvalidEndpoint(#[source] url::ParseError),
    #[error("endpoint must use HTTP(S)")]
    UnsupportedEndpointScheme,
    #[error("endpoint must have a host")]
    MissingEndpointHost,
    #[error("endpoint must not contain embedded credentials")]
    EndpointCredentials,
    #[error("endpoint must not contain a query")]
    EndpointQuery,
    #[error("endpoint must not contain a fragment")]
    EndpointFragment,
    #[error("workspace must be a directory")]
    WorkspaceNotDirectory,
    #[error("prompt cannot be empty")]
    EmptyPrompt,
}

/// Safe title-inference failures. Upstream failures may contain private input,
/// so these variants intentionally retain neither messages nor source errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SummaryError {
    #[error("the inference task stopped unexpectedly")]
    TaskStopped,
    #[error("cancelled")]
    Cancelled,
    #[error("the model returned no text")]
    NoText,
    #[error("inference failed")]
    InferenceFailed,
    #[error("inference timed out")]
    InferenceTimeout,
    #[error("session creation failed")]
    CreationFailed,
    #[error("session creation timed out")]
    CreationTimeout,
    #[error("temporary session deletion timed out")]
    DeletionTimeout,
    #[error("temporary session could not be deleted")]
    DeletionFailed,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid runtime configuration: {0}")]
    InvalidConfiguration(#[from] ConfigurationError),
    #[error("Copilot runtime: {0}")]
    Sdk(#[from] github_copilot_sdk::Error),
    #[error("runtime storage: {0}")]
    Io(#[from] std::io::Error),
    #[error("this session already has an active turn")]
    Busy,
    #[error("Copilot runtime is disconnected; start a new connection before sending")]
    Disconnected,
    #[error("Copilot runtime did not complete startup within thirty seconds")]
    StartupTimeout,
    #[error("Copilot runtime did not acknowledge cancellation within ten seconds")]
    AbortTimeout,
    #[error("approval request is missing, expired, or already answered")]
    UnknownApproval,
    #[error("failed to stop Copilot runtime: {0}")]
    Shutdown(#[source] github_copilot_sdk::StopErrors),
    #[error("title summary: {0}")]
    Summary(#[from] SummaryError),
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn underlying_storage_and_sdk_errors_remain_downcastable() {
        let storage: Error = std::io::Error::from(std::io::ErrorKind::PermissionDenied).into();
        assert!(matches!(
            &storage,
            Error::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied
        ));
        assert!(
            storage
                .source()
                .and_then(|source| source.downcast_ref::<std::io::Error>())
                .is_some()
        );

        let sdk: Error = github_copilot_sdk::Error::from(std::io::Error::from(
            std::io::ErrorKind::ConnectionReset,
        ))
        .into();
        assert!(matches!(sdk, Error::Sdk(_)));
        assert!(
            sdk.source()
                .and_then(|source| source.downcast_ref::<github_copilot_sdk::Error>())
                .and_then(std::error::Error::source)
                .and_then(|source| source.downcast_ref::<std::io::Error>())
                .is_some_and(|source| source.kind() == std::io::ErrorKind::ConnectionReset)
        );
    }
}
