use anyhow::{Result, bail};
pub use copilot_runtime::PermissionMode;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub name: String,
    pub reasoning_effort: Option<String>,
    pub context_window: Option<i64>,
    pub max_output_tokens: Option<i64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    OpenaiCompletions,
    OpenaiResponses,
    Anthropic,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    pub base_url: String,
    #[serde(default)]
    pub has_api_key: bool,
    pub models: Vec<Model>,
}

impl Provider {
    /// Validate provider input before storing it or handing it to the SDK.
    ///
    /// # Errors
    /// Returns an error for incomplete, duplicate, or unsupported configuration.
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.name, "供应商名称")?;
        validate_endpoint(&self.base_url)?;
        if self.models.is_empty() {
            bail!("请至少添加一个模型");
        }
        let mut ids = std::collections::HashSet::new();
        for model in &self.models {
            validate_name(&model.id, "模型 ID")?;
            validate_name(&model.name, "模型名称")?;
            if !ids.insert(&model.id) {
                bail!("模型 ID 不能重复：{}", model.id);
            }
            if let Some(effort) = &model.reasoning_effort
                && !["low", "medium", "high"].contains(&effort.as_str())
            {
                bail!("推理强度只能是 low、medium 或 high");
            }
            for count in [model.context_window, model.max_output_tokens]
                .into_iter()
                .flatten()
            {
                if !(1..=10_000_000).contains(&count) {
                    bail!("Token 数量必须在 1 到 10,000,000 之间");
                }
            }
            if let (Some(context), Some(output)) = (model.context_window, model.max_output_tokens)
                && output >= context
            {
                bail!("最大输出必须小于上下文窗口，为输入保留空间");
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
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "http://127.0.0.1:4318".into(),
            protocol: default_telemetry_protocol(),
            capture_content: false,
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
                bail!("Copilot SDK 支持 http/protobuf 和 http/json 遥测导出");
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
    #[serde(default)]
    pub title_model: Option<TitleModel>,
    pub telemetry: Telemetry,
    pub cli_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub title_model: Option<TitleModel>,
    pub providers: Vec<Provider>,
    pub telemetry: Telemetry,
    pub cli_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Connection {
    Local,
    Ssh {
        host: String,
        port: u16,
        user: String,
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
    #[serde(default)]
    pub title_state: TitleState,
    pub id: String,
    pub title: String,
    pub project_id: Option<String>,
    pub provider_id: String,
    pub model_id: String,
    #[serde(default)]
    pub permission_mode: PermissionMode,
    pub created_at: String,
    pub updated_at: String,
    pub status: SessionStatus,
}

/// Automatic titles are opt-in for newly created sessions; existing names stay intact.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TitleState {
    #[default]
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
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: MessageRole,
    pub content: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupted: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveTurnView {
    pub stream: String,
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
    pub title_generations: Vec<String>,
    pub settings: Settings,
    pub projects: Vec<Project>,
    pub sessions: Vec<Session>,
    pub runtime: RuntimeStatus,
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub active_turns: std::collections::HashMap<String, ActiveTurnView>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub session_id: String,
    pub kind: String,
    pub data: serde_json::Value,
}

pub(crate) fn validate_name(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() || value.chars().count() > 256 || value.chars().any(char::is_control)
    {
        bail!("{label}不能为空、不能包含控制字符，且最多为 256 个字符");
    }
    Ok(())
}

pub(crate) fn validate_endpoint(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(|_| anyhow::anyhow!("请输入完整的 HTTP 或 HTTPS 地址"))?;
    if !["http", "https"].contains(&url.scheme()) || url.host_str().is_none() {
        bail!("地址必须使用 HTTP 或 HTTPS");
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("地址不能包含凭证、查询参数或片段；请将密钥填入 API Key");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn stored_sessions_default_to_ask_and_preserve_explicit_permission_modes() -> Result<()> {
        for (field, expected) in [
            (None, PermissionMode::Ask),
            (Some("ask"), PermissionMode::Ask),
            (Some("allow-all"), PermissionMode::AllowAll),
        ] {
            let mut data = json!({
                "id":"session", "title":"Work", "projectId":"project",
                "providerId":"provider", "modelId":"model", "status":"idle",
                "createdAt":"2026-10-17", "updatedAt":"2026-10-17"
            });
            if let Some(mode) = field {
                data["permissionMode"] = json!(mode);
            }
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
            (Some(1_000_000), Some(128_000), true),
            (Some(1_000_000), None, true),
            (None, Some(128_000), true),
            (Some(128_000), Some(128_000), false),
            (Some(128_000), Some(128_001), false),
            (Some(0), Some(128_000), false),
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
}
