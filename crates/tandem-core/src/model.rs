use std::fmt;

pub use copilot_runtime::{ContextUsage, MessageAttachment, PermissionMode, SkillSelection};
use serde::{Deserialize, Serialize};
use url::Url;

/// A display-name field with consistent validation and a stable typed identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NameField {
    ProviderName,
    ModelId,
    ModelName,
    ProjectName,
    ConversationName,
}

impl fmt::Display for NameField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ProviderName => "Provider name",
            Self::ModelId => "Model ID",
            Self::ModelName => "Model name",
            Self::ProjectName => "Project name",
            Self::ConversationName => "Conversation name",
        })
    }
}

/// Invalid user-supplied provider, model, telemetry, or display-name settings.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("{field} must not be empty, contain control characters, or exceed 256 characters.")]
    InvalidName { field: NameField },
    #[error("Add at least one model.")]
    MissingModels,
    #[error(
        "Enter a relative model-list path without an origin, fragment, backslash, or dot segments."
    )]
    InvalidModelListPath,
    #[error("Model IDs must be unique: {id}")]
    DuplicateModelId { id: String },
    #[error("Reasoning effort must be low, medium, high, or max.")]
    UnsupportedReasoningEffort { effort: String },
    #[error("Token counts must be between 1 and 10,000,000.")]
    InvalidTokenCount { count: i64 },
    #[error("Maximum output must be smaller than the context window to leave room for input.")]
    OutputExceedsContext { context: i64, output: i64 },
    #[error("Enter a complete HTTP or HTTPS URL.")]
    InvalidEndpoint {
        #[source]
        source: url::ParseError,
    },
    #[error("The URL must use HTTP or HTTPS.")]
    UnsupportedEndpointScheme,
    #[error("The URL must not contain credentials. Enter credentials in API Key.")]
    EndpointContainsCredentials,
    #[error("The URL must not contain query parameters.")]
    EndpointContainsQuery,
    #[error("The URL must not contain fragments.")]
    EndpointContainsFragment,
    #[error("Copilot SDK telemetry export supports http/protobuf and http/json.")]
    UnsupportedTelemetryProtocol { protocol: String },
    #[error("Enter an AgentLens service name without control characters, up to 256 characters.")]
    InvalidAgentLensServiceName,
    #[error("Enter an AgentLens API key before enabling AgentLens telemetry.")]
    MissingAgentLensApiKey,
    #[error("Select HTTP / JSON for AgentLens telemetry.")]
    AgentLensRequiresJson,
}

type Result<T> = std::result::Result<T, ValidationError>;

pub(crate) fn validate_reasoning_effort(effort: Option<&str>) -> Result<()> {
    if let Some(effort) = effort
        && !["low", "medium", "high", "max"].contains(&effort)
    {
        return Err(ValidationError::UnsupportedReasoningEffort {
            effort: effort.into(),
        });
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub name: String,
    pub reasoning_effort: Option<String>,
    pub context_window: i64,
    pub max_output_tokens: Option<i64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    OpenaiCompletions,
    OpenaiResponses,
    Anthropic,
}

impl From<Protocol> for copilot_runtime::ProviderProtocol {
    fn from(protocol: Protocol) -> Self {
        match protocol {
            Protocol::OpenaiCompletions => Self::OpenaiCompletions,
            Protocol::OpenaiResponses => Self::OpenaiResponses,
            Protocol::Anthropic => Self::Anthropic,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    pub base_url: String,
    #[serde(default = "default_list_models_path")]
    pub list_models_path: String,
    #[serde(default)]
    pub has_api_key: bool,
    pub models: Vec<Model>,
}

pub(crate) fn default_list_models_path() -> String {
    "/v1/models".into()
}

impl Provider {
    /// Validate provider input before storing it or handing it to the SDK.
    ///
    /// # Errors
    /// Returns an error for incomplete, duplicate, or unsupported configuration.
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.name, NameField::ProviderName)?;
        validate_endpoint(&self.base_url)?;
        crate::model_discovery::validate_path(&self.list_models_path)?;
        if self.models.is_empty() {
            return Err(ValidationError::MissingModels);
        }
        let mut ids = std::collections::HashSet::new();
        for model in &self.models {
            validate_name(&model.id, NameField::ModelId)?;
            validate_name(&model.name, NameField::ModelName)?;
            if !ids.insert(&model.id) {
                return Err(ValidationError::DuplicateModelId {
                    id: model.id.clone(),
                });
            }
            validate_reasoning_effort(model.reasoning_effort.as_deref())?;
            for count in [Some(model.context_window), model.max_output_tokens]
                .into_iter()
                .flatten()
            {
                if !(1..=10_000_000).contains(&count) {
                    return Err(ValidationError::InvalidTokenCount { count });
                }
            }
            let context = model.context_window;
            if let Some(output) = model.max_output_tokens
                && output >= context
            {
                return Err(ValidationError::OutputExceedsContext { context, output });
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Telemetry {
    pub enabled: bool,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default = "default_telemetry_protocol")]
    pub protocol: String,
    #[serde(default)]
    pub capture_content: bool,
    #[serde(default)]
    pub agentlens: Option<AgentLensConfig>,
}

/// Public `AgentLens` configuration. The credential is stored separately.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLensConfig {
    pub service_name: String,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "http://127.0.0.1:4318".into(),
            protocol: default_telemetry_protocol(),
            capture_content: false,
            agentlens: None,
        }
    }
}

fn default_telemetry_protocol() -> String {
    "http/protobuf".into()
}

impl Telemetry {
    /// Check the endpoint and the export protocols supported by the SDK.
    ///
    /// # Errors
    /// Returns an error for invalid endpoints or unsupported protocols.
    pub fn validate(&self) -> Result<()> {
        if self.enabled {
            validate_endpoint(&self.endpoint)?;
            if !["http/protobuf", "http/json"].contains(&self.protocol.as_str()) {
                return Err(ValidationError::UnsupportedTelemetryProtocol {
                    protocol: self.protocol.clone(),
                });
            }
            if self.agentlens.as_ref().is_some_and(|config| {
                config.service_name.trim().is_empty()
                    || config.service_name.chars().count() > 256
                    || config.service_name.chars().any(char::is_control)
            }) {
                return Err(ValidationError::InvalidAgentLensServiceName);
            }
            if self.agentlens.is_some() && self.protocol != "http/json" {
                return Err(ValidationError::AgentLensRequiresJson);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleModel {
    pub provider_id: String,
    pub model_id: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    pub title_model: Option<TitleModel>,
    pub telemetry: Telemetry,
    pub cli_path: Option<String>,
    pub font_size: FontSize,
}

/// Application-wide text sizing, independent of the agent runtime.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FontSize {
    Small,
    #[default]
    Medium,
    Large,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub title_model: Option<TitleModel>,
    pub providers: Vec<Provider>,
    pub telemetry: Telemetry,
    pub cli_path: Option<String>,
    pub font_size: FontSize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Connection {
    Local,
    Ssh {
        host: String,
        port: Option<u16>,
        user: Option<String>,
        #[serde(rename = "identityFile")]
        identity_file: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub connection: Connection,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct DirectoryListing {
    pub path: String,
    pub entries: Vec<DirectoryEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub title_state: TitleState,
    pub id: String,
    pub title: String,
    pub project_id: Option<String>,
    pub provider_id: String,
    pub model_id: String,
    pub permission_mode: PermissionMode,
    /// None inherits the selected provider model's default.
    pub reasoning_effort: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub status: SessionStatus,
    pub context_usage: Option<ContextUsage>,
}

/// Persisted lifecycle of a session's automatic or user-selected title.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TitleState {
    Preserved,
    Pending,
    Attempted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Idle,
    Running,
    Error,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    Tool,
    System,
    Activity,
    Reasoning,
    Agent,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivityStatus {
    Running,
    Completed,
    Error,
    Interrupted,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolActivity {
    pub tool_call_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub status: ActivityStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub status: ActivityStatus,
    pub tools: Vec<ToolActivity>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reasoning {
    pub status: ActivityStatus,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentMetadata {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size: usize,
}

/// A worker's lifecycle, persisted independently from its transcript.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Subagent {
    pub id: String,
    pub tool_call_id: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub status: ActivityStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: MessageRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<Subagent>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fleet: bool,
    pub content: String,
    pub attachments: Vec<AttachmentMetadata>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_skills: Vec<SkillSelection>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<Activity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupted: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveTurnView {
    pub stream: String,
    pub agent_streams: std::collections::HashMap<String, String>,
    pub permissions: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub available: bool,
    pub version: Option<String>,
    pub message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub preparing_sessions: Vec<String>,
    pub title_generations: Vec<String>,
    pub settings: Settings,
    pub projects: Vec<Project>,
    pub sessions: Vec<Session>,
    pub runtime: RuntimeStatus,
    pub active_turns: std::collections::HashMap<String, ActiveTurnView>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub session_id: String,
    pub kind: String,
    pub data: serde_json::Value,
}

pub(crate) fn validate_name(value: &str, field: NameField) -> Result<()> {
    if value.trim().is_empty() || value.chars().count() > 256 || value.chars().any(char::is_control)
    {
        return Err(ValidationError::InvalidName { field });
    }
    Ok(())
}

pub(crate) fn validate_endpoint(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(|source| ValidationError::InvalidEndpoint { source })?;
    if !["http", "https"].contains(&url.scheme()) || url.host_str().is_none() {
        return Err(ValidationError::UnsupportedEndpointScheme);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ValidationError::EndpointContainsCredentials);
    }
    if url.query().is_some() {
        return Err(ValidationError::EndpointContainsQuery);
    }
    if url.fragment().is_some() {
        return Err(ValidationError::EndpointContainsFragment);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use anyhow::{Context, Result};
    use serde_json::json;

    use super::*;

    #[test]
    fn invalid_endpoints_preserve_categories_without_retaining_sensitive_input() -> Result<()> {
        let invalid = [
            "https://fixture-user:fixture-secret@[invalid",
            "https://fixture-user:fixture-secret@example.com",
            "https://example.com?key=fixture-secret",
            "https://example.com#fixture-secret",
            "file:///tmp/fixture-secret",
        ];
        for (index, endpoint) in invalid.into_iter().enumerate() {
            let error = validate_endpoint(endpoint)
                .err()
                .context("endpoint must be rejected")?;
            assert!(matches!(
                (&error, index),
                (ValidationError::InvalidEndpoint { .. }, 0)
                    | (ValidationError::EndpointContainsCredentials, 1)
                    | (ValidationError::EndpointContainsQuery, 2)
                    | (ValidationError::EndpointContainsFragment, 3)
                    | (ValidationError::UnsupportedEndpointScheme, 4)
            ));
            let diagnostic = format!("{error} {error:?}");
            assert!(!diagnostic.contains("fixture-secret"));
            assert!(!diagnostic.contains("fixture-user"));
            if index == 0 {
                let source = error
                    .source()
                    .context("URL parsing must retain its source")?;
                assert!(!source.to_string().contains("fixture-secret"));
                assert!(source.downcast_ref::<url::ParseError>().is_some());
            } else {
                assert!(error.source().is_none());
            }
        }
        Ok(())
    }

    #[test]
    fn name_boundaries_retain_their_typed_field() {
        for (value, valid) in [
            (String::new(), false),
            ("   ".into(), false),
            ("Name\n".into(), false),
            ("a".repeat(257), false),
            ("名字".repeat(128), true),
        ] {
            for field in [
                NameField::ProviderName,
                NameField::ModelId,
                NameField::ModelName,
                NameField::ProjectName,
                NameField::ConversationName,
            ] {
                let result = validate_name(&value, field);
                if valid {
                    assert!(result.is_ok());
                } else {
                    assert!(
                        matches!(result, Err(ValidationError::InvalidName { field: actual }) if actual == field)
                    );
                }
            }
        }
    }

    #[test]
    fn validation_failures_expose_domain_fields() -> Result<()> {
        assert!(matches!(
            validate_name("", NameField::ProjectName),
            Err(ValidationError::InvalidName {
                field: NameField::ProjectName
            })
        ));
        let provider: Provider = serde_json::from_value(json!({
            "id": "provider", "name": "Provider", "protocol": "anthropic",
            "baseUrl": "https://example.com", "models": [{"id": "model", "name": "Model", "contextWindow": 200_000}]
        }))?;
        let model = provider.models[0].clone();
        let mut missing = provider.clone();
        missing.models.clear();
        assert!(matches!(
            missing.validate(),
            Err(ValidationError::MissingModels)
        ));
        let mut duplicate = provider.clone();
        duplicate.models.push(model);
        assert!(matches!(
            duplicate.validate(),
            Err(ValidationError::DuplicateModelId { id }) if id == "model"
        ));
        let mut effort = provider.clone();
        effort.models[0].reasoning_effort = Some("unsupported".into());
        assert!(matches!(
            effort.validate(),
            Err(ValidationError::UnsupportedReasoningEffort { effort }) if effort == "unsupported"
        ));
        let mut tokens = provider.clone();
        tokens.models[0].context_window = 0;
        assert!(matches!(
            tokens.validate(),
            Err(ValidationError::InvalidTokenCount { count: 0 })
        ));
        let mut output = provider;
        output.models[0].context_window = 100;
        output.models[0].max_output_tokens = Some(101);
        assert!(matches!(
            output.validate(),
            Err(ValidationError::OutputExceedsContext {
                context: 100,
                output: 101
            })
        ));
        let telemetry = Telemetry {
            enabled: true,
            protocol: "grpc".into(),
            ..Telemetry::default()
        };
        assert!(matches!(
            telemetry.validate(),
            Err(ValidationError::UnsupportedTelemetryProtocol { protocol }) if protocol == "grpc"
        ));
        Ok(())
    }

    #[test]
    fn stored_sessions_require_explicit_title_and_permission_state() -> Result<()> {
        let data = json!({
            "id":"session", "title":"Work", "titleState":"pending", "projectId":"project",
            "providerId":"provider", "modelId":"model", "permissionMode":"ask", "status":"idle",
            "createdAt":"2026-10-17", "updatedAt":"2026-10-17"
        });
        for field in ["titleState", "permissionMode"] {
            let mut incomplete = data.clone();
            incomplete
                .as_object_mut()
                .context("session object missing")?
                .remove(field);
            assert!(serde_json::from_value::<Session>(incomplete).is_err());
        }
        for expected in [PermissionMode::Ask, PermissionMode::AllowAll] {
            let mut data = data.clone();
            data["permissionMode"] = serde_json::to_value(expected)?;
            let session: Session = serde_json::from_value(data)?;
            assert_eq!(session.permission_mode, expected);
            assert_eq!(
                serde_json::to_value(session)?["permissionMode"],
                serde_json::to_value(expected)?
            );
        }
        Ok(())
    }

    #[test]
    fn context_window_reserves_space_for_input() -> Result<()> {
        for (context, output, valid) in [
            (1_000_000, Some(128_000), true),
            (1_000_000, None, true),
            (128_000, Some(128_000), false),
            (128_000, Some(128_001), false),
            (0, Some(128_000), false),
            (-1, None, false),
            (10_000_001, None, false),
            (10_000_000, None, true),
        ] {
            let provider: Provider = serde_json::from_value(json!({
                "id": "", "name": "Provider", "protocol": "anthropic",
                "baseUrl": "https://example.com", "models": [{
                    "id": "model", "name": "Model", "contextWindow": context,
                    "maxOutputTokens": output,
                }],
            }))?;
            assert_eq!(provider.validate().is_ok(), valid, "{context:?}/{output:?}");
        }
        Ok(())
    }

    #[test]
    fn model_configuration_requires_a_context_window() {
        for data in [
            json!({"id":"model","name":"Model"}),
            json!({"id":"model","name":"Model","contextWindow":null}),
        ] {
            assert!(serde_json::from_value::<Model>(data).is_err());
        }
    }
}
