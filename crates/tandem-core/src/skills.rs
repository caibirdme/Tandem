use std::path::Path;

use copilot_runtime::ProjectSkill;
use serde_json::json;

use crate::{App, Error, Result, model::Connection};

impl App {
    pub(crate) async fn list_project_skills(&self, project_id: &str) -> Result<Vec<ProjectSkill>> {
        let project = self.store.project(project_id)?;
        match project.connection {
            Connection::Local => Ok(self
                .ensure_runtime()
                .await?
                .list_project_skills(Path::new(&project.path))
                .await?),
            Connection::Ssh { .. } => {
                let transport = self.remote_client(&project.connection)?.connect().await?;
                let response = transport
                    .request("list_project_skills", json!({"directory": project.path}))
                    .await;
                transport.shutdown().await;
                let response = response?;
                serde_json::from_value(response)
                    .map_err(|source| Error::InvalidSkillCatalog(source.into()))
            }
        }
    }
}
