use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    App,
    model::{Connection, PermissionMode, Provider, Telemetry, TitleModel},
};

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
    DeleteProvider {
        id: String,
    },
    GetProviderApiKey {
        id: String,
    },
    SavePreferences {
        telemetry: Telemetry,
        cli_path: Option<String>,
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
    CreateSession {
        project_id: Option<String>,
        provider_id: String,
        model_id: String,
        #[serde(default)]
        permission_mode: PermissionMode,
    },
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
    Messages {
        session_id: String,
    },
    SendMessage {
        session_id: String,
        content: String,
        message_id: Option<Uuid>,
    },
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
            Request::GetProviderApiKey { id } => response(self.get_provider_api_key(&id)?),
            Request::SavePreferences {
                telemetry,
                cli_path,
            } => response(self.save_preferences(telemetry, cli_path).await?),
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
            Request::CreateSession {
                project_id,
                provider_id,
                model_id,
                permission_mode,
            } => response(
                self.create_session(project_id, provider_id, model_id, permission_mode)
                    .await?,
            ),
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
            Request::Messages { session_id } => response(self.store.messages(&session_id)?),
            Request::SendMessage {
                session_id,
                content,
                message_id,
            } => response(self.send_message(&session_id, content, message_id).await?),
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
            Request::BrowseDirectory { connection, path } => {
                response(self.browse_directory(&connection, &path).await?)
            }
            Request::InstallRuntime {} => {
                self.ensure_runtime().await?;
                response(self.runtime_status().await)
            }
        }
    }
}

fn response(value: impl Serialize) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}
