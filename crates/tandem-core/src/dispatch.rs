use std::sync::Arc;

use copilot_runtime::{MessageAttachment, SkillSelection, workspace::ReviewSelection};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    App, Result,
    model::{Connection, FontSize, PermissionMode, Provider, Session, Telemetry, TitleModel},
};

/// Conversation creation settings at the application command boundary.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    pub project_id: Option<String>,
    pub provider_id: String,
    pub model_id: String,
    pub permission_mode: Option<PermissionMode>,
    pub reasoning_effort: Option<String>,
}

impl CreateSessionRequest {
    async fn execute(self, app: &App) -> Result<Session> {
        app.create_session(
            self.project_id,
            self.provider_id,
            self.model_id,
            self.permission_mode,
            self.reasoning_effort,
        )
        .await
    }
}

/// One user turn, with Fleet available only through explicit opt-in.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageRequest {
    pub session_id: String,
    pub content: String,
    pub message_id: Uuid,
    #[serde(default)]
    pub selected_skills: Vec<SkillSelection>,
    #[serde(default)]
    pub attachments: Vec<MessageAttachment>,
    #[serde(default)]
    pub fleet: bool,
}

impl SendMessageRequest {
    async fn execute(self, app: &Arc<App>) -> Result<()> {
        app.send_message(
            &self.session_id,
            self.content,
            self.message_id,
            &self.selected_skills,
            &self.attachments,
            self.fleet,
        )
        .await
    }
}

/// A current or historical review selection within one session.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSummaryRequest {
    pub session_id: String,
    pub selection: ReviewSelection,
}

impl ReviewSummaryRequest {
    async fn execute(self, app: &App) -> Result<Value> {
        response(
            app.get_review_summary(&self.session_id, self.selection)
                .await?,
        )
    }
}

/// One metadata page from an immutable review revision.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFilesRequest {
    pub session_id: String,
    pub review_id: String,
    pub revision: String,
    #[serde(default)]
    pub cursor: usize,
}

impl ReviewFilesRequest {
    async fn execute(self, app: &App) -> Result<Value> {
        response(
            app.list_review_files(
                &self.session_id,
                &self.review_id,
                &self.revision,
                self.cursor,
            )
            .await?,
        )
    }
}

/// One file from an immutable review revision.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFileRequest {
    pub session_id: String,
    pub review_id: String,
    pub revision: String,
    pub file_id: String,
}

impl ReviewFileRequest {
    async fn execute(self, app: &App) -> Result<Value> {
        response(
            app.get_review_file(
                &self.session_id,
                &self.review_id,
                &self.revision,
                &self.file_id,
            )
            .await?,
        )
    }
}

/// Stable command contract, used by both Tauri IPC and the development server.
#[derive(Deserialize)]
#[serde(
    tag = "command",
    content = "payload",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Request {
    Snapshot {},
    SaveProvider {
        provider: Provider,
        api_key: Option<String>,
    },
    ListProviderModels {
        base_url: String,
        list_models_path: String,
        api_key: Option<String>,
    },
    DeleteProvider {
        id: String,
    },
    GetProviderApiKey {
        id: String,
    },
    GetTelemetryApiKey {},
    SavePreferences {
        telemetry: Telemetry,
        cli_path: Option<String>,
        telemetry_api_key: Option<String>,
    },
    SaveFontSize {
        font_size: FontSize,
    },
    SaveTitleModel {
        title_model: Option<TitleModel>,
    },
    RegenerateTitle {
        session_id: String,
    },
    CreateProject {
        name: String,
        path: String,
        connection: Connection,
    },
    DeleteProject {
        id: String,
    },
    ListProjectSkills {
        project_id: String,
    },
    CreateSession(CreateSessionRequest),
    DeleteSession {
        session_id: String,
    },
    RenameSession {
        session_id: String,
        title: String,
    },
    SetPermissionMode {
        session_id: String,
        permission_mode: PermissionMode,
    },
    SetSessionModel {
        session_id: String,
        provider_id: String,
        model_id: String,
    },
    SetSessionReasoningEffort {
        session_id: String,
        #[serde(deserialize_with = "deserialize_reasoning_effort")]
        reasoning_effort: Option<String>,
    },
    Messages {
        session_id: String,
    },
    GetAttachment {
        session_id: String,
        attachment_id: String,
    },
    GetReviewSummary(ReviewSummaryRequest),
    RefreshReview(ReviewSummaryRequest),
    ListReviewFiles(ReviewFilesRequest),
    GetReviewFile(ReviewFileRequest),
    SendMessage(SendMessageRequest),
    AbortSession {
        session_id: String,
    },
    RespondPermission {
        session_id: String,
        request_id: String,
        approved: bool,
    },
    CheckConnection {
        connection: Connection,
    },
    ListSshHosts {},
    BrowseDirectory {
        connection: Connection,
        path: String,
    },
    InstallRuntime {},
}

impl App {
    /// Execute a validated application command. No transport-specific state leaks here.
    ///
    /// # Errors
    /// Returns configuration, persistence, connectivity, or runtime errors for display.
    pub async fn dispatch(self: &Arc<Self>, request: Request) -> Result<Value> {
        match request {
            Request::Snapshot {} => response(self.snapshot().await?),
            Request::SaveProvider { provider, api_key } => {
                response(self.save_provider(provider, api_key).await?)
            }
            Request::DeleteProvider { id } => response(self.delete_provider(&id).await?),
            Request::ListProviderModels {
                base_url,
                list_models_path,
                api_key,
            } => provider_models_response(&base_url, &list_models_path, api_key.as_deref()).await,
            Request::GetProviderApiKey { id } => response(self.get_provider_api_key(&id)?),
            Request::GetTelemetryApiKey {} => response(self.get_telemetry_api_key()?),
            Request::SavePreferences {
                telemetry,
                cli_path,
                telemetry_api_key,
            } => response(
                self.save_preferences(telemetry, cli_path, telemetry_api_key)
                    .await?,
            ),
            Request::SaveFontSize { font_size } => response(self.save_font_size(font_size).await?),
            Request::SaveTitleModel { title_model } => {
                response(self.save_title_model(title_model).await?)
            }
            Request::RegenerateTitle { session_id } => {
                response(self.schedule_title(&session_id, true)?)
            }
            Request::CreateProject {
                name,
                path,
                connection,
            } => response(self.create_project(name, path, connection).await?),
            Request::DeleteProject { id } => response(self.delete_project(&id).await?),
            Request::ListProjectSkills { project_id } => {
                response(self.list_project_skills(&project_id).await?)
            }
            Request::CreateSession(request) => response(request.execute(self).await?),
            Request::DeleteSession { session_id } => {
                response(self.delete_session(&session_id).await?)
            }
            Request::RenameSession { session_id, title } => {
                response(self.rename_session(&session_id, &title).await?)
            }
            Request::SetPermissionMode {
                session_id,
                permission_mode,
            } => response(
                self.set_permission_mode(&session_id, permission_mode)
                    .await?,
            ),
            Request::SetSessionModel {
                session_id,
                provider_id,
                model_id,
            } => response(
                self.set_session_model(&session_id, provider_id, model_id)
                    .await?,
            ),
            Request::SetSessionReasoningEffort {
                session_id,
                reasoning_effort,
            } => response(
                self.set_session_reasoning_effort(&session_id, reasoning_effort)
                    .await?,
            ),
            Request::Messages { session_id } => response(self.messages(&session_id)?),
            Request::GetAttachment {
                session_id,
                attachment_id,
            } => response(self.store.attachment(&session_id, &attachment_id)?),
            Request::GetReviewSummary(request) | Request::RefreshReview(request) => {
                request.execute(self).await
            }
            Request::ListReviewFiles(request) => request.execute(self).await,
            Request::GetReviewFile(request) => request.execute(self).await,
            Request::SendMessage(request) => response(request.execute(self).await?),
            Request::AbortSession { session_id } => {
                response(self.abort_session(&session_id).await?)
            }
            Request::RespondPermission {
                session_id,
                request_id,
                approved,
            } => response(
                self.respond_permission(&session_id, &request_id, approved)
                    .await?,
            ),
            Request::CheckConnection { connection } => {
                Ok(json!({"message":self.check_connection(&connection).await?}))
            }
            Request::ListSshHosts {} => response(Self::list_ssh_hosts().await?),
            Request::BrowseDirectory { connection, path } => {
                response(self.browse_directory(&connection, &path).await?)
            }
            Request::InstallRuntime {} => response(self.install_runtime().await?),
        }
    }
}

async fn provider_models_response(
    base_url: &str,
    list_models_path: &str,
    api_key: Option<&str>,
) -> Result<Value> {
    response(crate::model_discovery::list_models(base_url, list_models_path, api_key).await?)
}

fn response(value: impl Serialize) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

// Requiring the field distinguishes an explicit reset from an incomplete update.
fn deserialize_reasoning_effort<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)
}
