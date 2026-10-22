//! Application settings mapped to the pinned SDK's supported configuration.

use std::{fmt, path::PathBuf};

use github_copilot_sdk::{
    OtelExporterType, OtlpHttpProtocol, ProviderConfig, TelemetryConfig,
    rpc::{ModelCapabilitiesOverride, ModelCapabilitiesOverrideLimits},
};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{ConfigurationError, Result};

/// Supported inference wire protocols.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderProtocol {
    OpenaiCompletions,
    OpenaiResponses,
    Anthropic,
}

/// Inference connection. Debug output deliberately omits the credential.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettings {
    pub protocol: ProviderProtocol,
    pub base_url: String,
    pub api_key: Option<String>,
}

impl fmt::Debug for ProviderSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderSettings")
            .field("protocol", &self.protocol)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

/// Per-model options supported by the pinned SDK.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSettings {
    pub id: String,
    pub reasoning_effort: Option<String>,
    /// Total input and output capacity.
    pub context_window: i64,
    pub max_output_tokens: Option<i64>,
}

impl ModelSettings {
    fn token_limits(&self) -> Result<ModelCapabilitiesOverrideLimits> {
        if self.context_window <= 0 {
            return Err(ConfigurationError::NonPositiveContextWindow.into());
        }
        if self.max_output_tokens.is_some_and(|limit| limit <= 0) {
            return Err(ConfigurationError::NonPositiveMaxOutputTokens.into());
        }
        // Reserve an explicit output allowance within the total window. When it
        // is omitted, preserve the SDK's output default and override the total
        // context separately, without retaining a smaller inferred prompt limit.
        let prompt = self.context_window - self.max_output_tokens.unwrap_or(0);
        if prompt <= 0 {
            return Err(ConfigurationError::OutputExceedsContextWindow.into());
        }
        Ok(ModelCapabilitiesOverrideLimits {
            max_context_window_tokens: Some(self.context_window),
            max_prompt_tokens: Some(prompt),
            max_output_tokens: self.max_output_tokens,
            ..Default::default()
        })
    }

    pub(crate) fn to_sdk_capabilities(&self) -> Result<Option<ModelCapabilitiesOverride>> {
        let limits = self.token_limits()?;
        Ok(Some(ModelCapabilitiesOverride {
            limits: Some(limits),
            ..Default::default()
        }))
    }
}

/// Capability boundary: chat can fetch URLs; work can operate in a directory.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SessionMode {
    Chat,
    Work { directory: PathBuf },
}

/// How work sessions answer SDK permission requests. Chat always approves automatically.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    #[default]
    Ask,
    AllowAll,
}

/// All configuration must be re-supplied when resuming a persisted SDK session.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionOptions {
    pub session_id: Option<String>,
    pub mode: SessionMode,
    pub permission_mode: PermissionMode,
    pub provider: ProviderSettings,
    pub model: ModelSettings,
    pub system_prompt: Option<String>,
}

/// The native runtime supports OTLP over HTTP, in these two encodings.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TelemetryProtocol {
    #[serde(rename = "http/json")]
    HttpJson,
    #[serde(rename = "http/protobuf")]
    HttpProtobuf,
}

/// Export configuration, applied when starting the SDK child process.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetrySettings {
    pub endpoint: String,
    pub protocol: TelemetryProtocol,
    #[serde(default)]
    pub capture_content: bool,
    #[serde(default)]
    pub agentlens: Option<AgentLensSettings>,
}

/// `AgentLens` resource configuration. Credentials are never included in diagnostics.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLensSettings {
    pub service_name: String,
    pub api_key: String,
}

impl fmt::Debug for AgentLensSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentLensSettings")
            .field("service_name", &self.service_name)
            .field("api_key", &"[redacted]")
            .finish()
    }
}

/// State is private to this application, independently of the user's CLI state.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeOptions {
    pub state_directory: PathBuf,
    /// Overrides the SDK's bundled, version-matched runtime executable.
    pub cli_path: Option<PathBuf>,
    pub telemetry: Option<TelemetrySettings>,
}

impl ProviderSettings {
    pub(crate) fn to_sdk(&self, model: &ModelSettings) -> Result<ProviderConfig> {
        validate_http_endpoint(&self.base_url)?;
        if model.id.trim().is_empty() {
            return Err(ConfigurationError::EmptyModelId.into());
        }
        if model
            .reasoning_effort
            .as_deref()
            .is_some_and(|effort| !["low", "medium", "high", "max"].contains(&effort))
        {
            return Err(ConfigurationError::UnsupportedReasoningEffort.into());
        }
        let limits = model.token_limits()?;
        let (provider_type, wire_api) = match self.protocol {
            ProviderProtocol::OpenaiCompletions => ("openai", Some("completions")),
            ProviderProtocol::OpenaiResponses => ("openai", Some("responses")),
            ProviderProtocol::Anthropic => ("anthropic", None),
        };
        let mut provider = ProviderConfig::default();
        provider.provider_type = Some(provider_type.into());
        provider.wire_api = wire_api.map(str::to_owned);
        self.base_url
            .trim_end_matches('/')
            .clone_into(&mut provider.base_url);
        provider.api_key.clone_from(&self.api_key);
        provider.max_prompt_tokens = limits.max_prompt_tokens;
        provider.max_output_tokens = limits.max_output_tokens;
        Ok(provider)
    }
}

impl TelemetrySettings {
    pub(crate) fn to_sdk(&self) -> Result<TelemetryConfig> {
        validate_http_endpoint(&self.endpoint)?;
        let protocol = match self.protocol {
            TelemetryProtocol::HttpJson => OtlpHttpProtocol::HttpJson,
            TelemetryProtocol::HttpProtobuf => OtlpHttpProtocol::HttpProtobuf,
        };
        Ok(TelemetryConfig::new()
            .with_otlp_endpoint(&self.endpoint)
            .with_otlp_protocol(protocol)
            .with_exporter_type(OtelExporterType::OtlpHttp)
            .with_capture_content(self.capture_content)
            .with_source_name("tandem"))
    }
}

fn validate_http_endpoint(endpoint: &str) -> Result<()> {
    let url = Url::parse(endpoint).map_err(ConfigurationError::InvalidEndpoint)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ConfigurationError::UnsupportedEndpointScheme.into());
    }
    if url.host_str().is_none() {
        return Err(ConfigurationError::MissingEndpointHost.into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ConfigurationError::EndpointCredentials.into());
    }
    if url.query().is_some() {
        return Err(ConfigurationError::EndpointQuery.into());
    }
    if url.fragment().is_some() {
        return Err(ConfigurationError::EndpointFragment.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;
    use crate::Error;

    #[test]
    fn remote_permission_mode_is_required_and_round_trips_explicit_choices()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut payload = serde_json::json!({
            "mode": {"kind": "work", "directory": "/project"},
            "provider": {"protocol": "openai-responses", "baseUrl": "https://example.com"},
            "model": {"id": "example-model", "contextWindow": 200_000}
        });
        assert!(serde_json::from_value::<SessionOptions>(payload.clone()).is_err());
        for (mode, wire) in [
            (PermissionMode::Ask, "ask"),
            (PermissionMode::AllowAll, "allow-all"),
        ] {
            payload["permissionMode"] = serde_json::json!(wire);
            let options: SessionOptions = serde_json::from_value(payload.clone())?;
            assert_eq!(options.permission_mode, mode);
            let encoded = serde_json::to_value(&options)?;
            assert_eq!(encoded["permissionMode"], wire);
            let restored: SessionOptions = serde_json::from_value(encoded)?;
            assert_eq!(restored.permission_mode, mode);
        }
        Ok(())
    }

    #[test]
    fn remote_reasoning_effort_rejects_unsupported_settings() -> Result<()> {
        let provider = ProviderSettings {
            protocol: ProviderProtocol::OpenaiResponses,
            base_url: "https://example.com/v1".into(),
            api_key: None,
        };
        for effort in [None, Some("low"), Some("medium"), Some("high"), Some("max")] {
            let model = ModelSettings {
                id: "fixture".into(),
                context_window: 200_000,
                reasoning_effort: effort.map(str::to_owned),
                max_output_tokens: None,
            };
            provider.to_sdk(&model)?;
        }
        for effort in ["", "LOW", " high", "xhigh", "unsupported"] {
            let model = ModelSettings {
                id: "fixture".into(),
                context_window: 200_000,
                reasoning_effort: Some(effort.into()),
                max_output_tokens: None,
            };
            assert!(matches!(
                provider.to_sdk(&model),
                Err(Error::InvalidConfiguration(
                    ConfigurationError::UnsupportedReasoningEffort
                ))
            ));
        }
        Ok(())
    }

    #[test]
    fn total_context_and_output_budget_are_distinct() -> Result<()> {
        for (context, output, prompt) in [
            (1_000_000, Some(128_000), Some(872_000)),
            (1_000_000, None, Some(1_000_000)),
        ] {
            let model = ModelSettings {
                context_window: context,
                max_output_tokens: output,
                ..Default::default()
            };
            let limits = model.token_limits()?;
            assert_eq!(limits.max_context_window_tokens, Some(context));
            assert_eq!(limits.max_prompt_tokens, prompt);
            assert_eq!(limits.max_output_tokens, output);
            assert!(model.to_sdk_capabilities()?.is_some());
        }
        for (context, output, expected) in [
            (0, None, ConfigurationError::NonPositiveContextWindow),
            (-1, None, ConfigurationError::NonPositiveContextWindow),
            (
                200_000,
                Some(0),
                ConfigurationError::NonPositiveMaxOutputTokens,
            ),
            (
                200_000,
                Some(-1),
                ConfigurationError::NonPositiveMaxOutputTokens,
            ),
            (
                128_000,
                Some(128_000),
                ConfigurationError::OutputExceedsContextWindow,
            ),
            (
                128_000,
                Some(128_001),
                ConfigurationError::OutputExceedsContextWindow,
            ),
        ] {
            let model = ModelSettings {
                context_window: context,
                max_output_tokens: output,
                ..Default::default()
            };
            assert!(
                matches!(model.token_limits(), Err(Error::InvalidConfiguration(error)) if error == expected),
                "{context:?} {output:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn model_settings_use_the_current_context_window_field()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let model: ModelSettings = serde_json::from_value(serde_json::json!({
            "id":"custom-model","contextWindow":1_000_000,"maxOutputTokens":128_000,
        }))?;
        assert_eq!(model.token_limits()?.max_prompt_tokens, Some(872_000));
        let encoded = serde_json::to_value(model)?;
        assert_eq!(encoded["contextWindow"], 1_000_000);
        assert!(encoded.get("maxPromptTokens").is_none());
        for data in [
            serde_json::json!({"id":"custom-model"}),
            serde_json::json!({"id":"custom-model","contextWindow":null}),
        ] {
            assert!(serde_json::from_value::<ModelSettings>(data).is_err());
        }
        assert!(
            serde_json::from_value::<ModelSettings>(serde_json::json!({
                "id":"custom-model","maxPromptTokens":1_000_000,
            }))
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn protocol_mapping_preserves_provider_and_wire_format() -> Result<()> {
        for (protocol, provider_type, wire_api) in [
            (
                ProviderProtocol::OpenaiCompletions,
                "openai",
                Some("completions"),
            ),
            (
                ProviderProtocol::OpenaiResponses,
                "openai",
                Some("responses"),
            ),
            (ProviderProtocol::Anthropic, "anthropic", None),
        ] {
            let settings = ProviderSettings {
                protocol,
                base_url: "http://localhost:11434/v1".into(),
                api_key: Some("secret-for-test".into()),
            };
            let config = settings.to_sdk(&ModelSettings {
                id: "test-model".into(),
                context_window: 200_000,
                ..Default::default()
            })?;
            assert_eq!(config.provider_type.as_deref(), Some(provider_type));
            assert_eq!(config.wire_api.as_deref(), wire_api);
            assert!(!format!("{settings:?}").contains("secret-for-test"));
        }
        Ok(())
    }

    #[test]
    fn provider_base_url_preserves_the_configured_path() -> Result<()> {
        for (protocol, endpoint, expected) in [
            (
                ProviderProtocol::Anthropic,
                "https://api.example.com/v1/",
                "https://api.example.com/v1",
            ),
            (
                ProviderProtocol::Anthropic,
                "https://api.example.com/gateway/v1",
                "https://api.example.com/gateway/v1",
            ),
            (
                ProviderProtocol::Anthropic,
                "https://api.example.com/gateway",
                "https://api.example.com/gateway",
            ),
            (
                ProviderProtocol::OpenaiResponses,
                "https://api.example.com/v1/",
                "https://api.example.com/v1",
            ),
            (
                ProviderProtocol::OpenaiCompletions,
                "https://api.example.com/gateway/",
                "https://api.example.com/gateway",
            ),
        ] {
            let provider = ProviderSettings {
                protocol,
                base_url: endpoint.into(),
                api_key: None,
            };
            let model = ModelSettings {
                id: "test-model".into(),
                context_window: 200_000,
                ..Default::default()
            };
            assert_eq!(provider.to_sdk(&model)?.base_url, expected);
        }
        Ok(())
    }

    #[test]
    fn endpoint_validation_rejects_ambiguous_or_credential_bearing_urls() {
        for endpoint in ["https://api.example.com/v1", "http://127.0.0.1:4318"] {
            assert!(validate_http_endpoint(endpoint).is_ok(), "{endpoint}");
        }
        for (endpoint, expected) in [
            (
                "file:///etc/passwd",
                ConfigurationError::UnsupportedEndpointScheme,
            ),
            (
                "api.example.com",
                ConfigurationError::InvalidEndpoint(url::ParseError::RelativeUrlWithoutBase),
            ),
            (
                "https://user:password@example.com",
                ConfigurationError::EndpointCredentials,
            ),
            (
                "https://example.com?key=secret",
                ConfigurationError::EndpointQuery,
            ),
            (
                "https://example.com/#fragment",
                ConfigurationError::EndpointFragment,
            ),
        ] {
            assert!(
                matches!(validate_http_endpoint(endpoint), Err(Error::InvalidConfiguration(error)) if error == expected),
                "{endpoint}"
            );
        }
    }

    #[test]
    fn endpoint_parse_errors_retain_the_parser_source_without_retaining_the_url() {
        let Err(error) = validate_http_endpoint("https://private-secret:invalid-port") else {
            panic!("invalid port must fail validation");
        };
        assert!(matches!(
            &error,
            Error::InvalidConfiguration(ConfigurationError::InvalidEndpoint(
                url::ParseError::InvalidPort
            ))
        ));
        assert!(
            error
                .source()
                .and_then(std::error::Error::source)
                .and_then(|source| source.downcast_ref::<url::ParseError>())
                .is_some()
        );
        assert!(!format!("{error:?}").contains("private-secret"));
    }
}
