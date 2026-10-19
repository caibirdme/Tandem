//! Application settings mapped to the pinned SDK's supported configuration.

use std::{fmt, path::PathBuf};

use github_copilot_sdk::{
    OtelExporterType, OtlpHttpProtocol, ProviderConfig, TelemetryConfig,
    rpc::{ModelCapabilitiesOverride, ModelCapabilitiesOverrideLimits},
};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{Error, Result};

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
    pub context_window: Option<i64>,
    pub max_output_tokens: Option<i64>,
}

impl ModelSettings {
    fn token_limits(&self) -> Result<ModelCapabilitiesOverrideLimits> {
        if self.context_window.is_some_and(|limit| limit <= 0)
            || self.max_output_tokens.is_some_and(|limit| limit <= 0)
        {
            return Err(Error::InvalidConfiguration(
                "token limits must be positive".into(),
            ));
        }
        // Reserve an explicit output allowance within the total window. When it
        // is omitted, preserve the SDK's output default and override the total
        // context separately, without retaining a smaller inferred prompt limit.
        let prompt = self
            .context_window
            .map(|context| context - self.max_output_tokens.unwrap_or(0));
        if prompt.is_some_and(|limit| limit <= 0) {
            return Err(Error::InvalidConfiguration(
                "maximum output tokens must be smaller than the context window".into(),
            ));
        }
        Ok(ModelCapabilitiesOverrideLimits {
            max_context_window_tokens: self.context_window,
            max_prompt_tokens: prompt,
            max_output_tokens: self.max_output_tokens,
            ..Default::default()
        })
    }

    pub(crate) fn to_sdk_capabilities(&self) -> Result<Option<ModelCapabilitiesOverride>> {
        let limits = self.token_limits()?;
        Ok(
            (self.context_window.is_some() || self.max_output_tokens.is_some()).then_some(
                ModelCapabilitiesOverride {
                    limits: Some(limits),
                    ..Default::default()
                },
            ),
        )
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
            return Err(Error::InvalidConfiguration(
                "model ID cannot be empty".into(),
            ));
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
        let base_url = self.base_url.trim_end_matches('/');
        match self.protocol {
            ProviderProtocol::Anthropic => base_url.strip_suffix("/v1").unwrap_or(base_url),
            ProviderProtocol::OpenaiCompletions | ProviderProtocol::OpenaiResponses => base_url,
        }
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
    let url = Url::parse(endpoint).map_err(|_| {
        Error::InvalidConfiguration("endpoint must be an absolute HTTP(S) URL".into())
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::InvalidConfiguration(
            "endpoint must use HTTP(S), without embedded credentials, query, or fragment".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_permission_mode_is_required_and_round_trips_explicit_choices()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut payload = serde_json::json!({
            "mode": {"kind": "work", "directory": "/project"},
            "provider": {"protocol": "openai-responses", "baseUrl": "https://example.com"},
            "model": {"id": "example-model"}
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
    fn total_context_and_output_budget_are_distinct() -> Result<()> {
        for (context, output, prompt) in [
            (Some(1_000_000), Some(128_000), Some(872_000)),
            (Some(1_000_000), None, Some(1_000_000)),
            (None, Some(128_000), None),
            (None, None, None),
        ] {
            let model = ModelSettings {
                context_window: context,
                max_output_tokens: output,
                ..Default::default()
            };
            let limits = model.token_limits()?;
            assert_eq!(limits.max_context_window_tokens, context);
            assert_eq!(limits.max_prompt_tokens, prompt);
            assert_eq!(limits.max_output_tokens, output);
            assert_eq!(
                model.to_sdk_capabilities()?.is_some(),
                context.is_some() || output.is_some()
            );
        }
        for (context, output) in [
            (Some(0), None),
            (Some(-1), None),
            (None, Some(0)),
            (None, Some(-1)),
            (Some(128_000), Some(128_000)),
            (Some(128_000), Some(128_001)),
        ] {
            let model = ModelSettings {
                context_window: context,
                max_output_tokens: output,
                ..Default::default()
            };
            assert!(model.token_limits().is_err(), "{context:?} {output:?}");
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
                ..Default::default()
            })?;
            assert_eq!(config.provider_type.as_deref(), Some(provider_type));
            assert_eq!(config.wire_api.as_deref(), wire_api);
            assert!(!format!("{settings:?}").contains("secret-for-test"));
        }
        Ok(())
    }

    #[test]
    fn anthropic_base_url_avoids_duplicate_version_paths() -> Result<()> {
        for (protocol, endpoint, expected) in [
            (
                ProviderProtocol::Anthropic,
                "https://api.example.com/v1/",
                "https://api.example.com",
            ),
            (
                ProviderProtocol::Anthropic,
                "https://api.example.com/gateway/v1",
                "https://api.example.com/gateway",
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
        ] {
            let provider = ProviderSettings {
                protocol,
                base_url: endpoint.into(),
                api_key: None,
            };
            let model = ModelSettings {
                id: "test-model".into(),
                ..Default::default()
            };
            assert_eq!(provider.to_sdk(&model)?.base_url, expected);
        }
        Ok(())
    }

    #[test]
    fn endpoint_validation_rejects_ambiguous_or_credential_bearing_urls() {
        for (endpoint, valid) in [
            ("https://api.example.com/v1", true),
            ("http://127.0.0.1:4318", true),
            ("file:///etc/passwd", false),
            ("api.example.com", false),
            ("https://user:password@example.com", false),
            ("https://example.com?key=secret", false),
            ("https://example.com/#fragment", false),
        ] {
            assert_eq!(
                validate_http_endpoint(endpoint).is_ok(),
                valid,
                "{endpoint}"
            );
        }
    }
}
