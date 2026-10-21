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
    #[error("reasoning effort must be low, medium, or high")]
    UnsupportedReasoningEffort,
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
    #[error("prepared message belongs to a different session; prepare it again for this session")]
    PreparedForDifferentSession,
    #[error("Multi-agent mode is only available in Work sessions")]
    FleetUnavailableInChat,
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

/// Explicit project skill selection failures, without retaining skill contents.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum SkillError {
    #[error("skills are only available in Work sessions")]
    NotAvailableInChat,
    #[error("select at most {limit} skills for one message")]
    TooManySelections { limit: usize },
    #[error("skill '{name}' was selected more than once")]
    DuplicateSelection { name: String },
    #[error("skill '{name}' is no longer available in this project; refresh the skill list")]
    Unavailable { name: String },
    #[error("skill '{name}' cannot be invoked as a message in this session")]
    UnsupportedInvocation { name: String },
    #[error("project skill preparation did not complete within fifteen seconds")]
    PreparationTimeout,
    #[error(
        "project skill discovery did not complete within fifteen seconds; reopen the skill list to retry"
    )]
    DiscoveryTimeout,
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
    #[error(
        "Copilot could not enable multi-agent tool permissions. Reconnect the session and try again."
    )]
    FleetToolsRejected,
    #[error("Copilot did not start multi-agent mode. Reconnect the session and try again.")]
    FleetNotStarted,
    #[error("Cannot encode the multi-agent request: {0}")]
    Serialization(#[from] serde_json::Error),
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
    #[error("project skills: {0}")]
    Skills(#[from] SkillError),
    #[error("message attachments: {0}")]
    Attachments(#[from] crate::AttachmentError),
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
