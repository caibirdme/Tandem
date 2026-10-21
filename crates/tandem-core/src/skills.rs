use std::{path::Path, sync::Arc};

use copilot_runtime::ProjectSkill;
use serde_json::json;
use tandem_remote::RemoteTransport;
use tokio::sync::OwnedMutexGuard;

use crate::{
    App, Error, Result,
    agent::AgentHandle,
    model::{Connection, SessionStatus},
};

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
                let (transport, session_guard) =
                    match self.project_skill_transport(project_id).await? {
                        Some((transport, guard)) => (transport, Some(guard)),
                        None => (
                            Arc::new(self.remote_client(&project.connection)?.connect().await?),
                            None,
                        ),
                    };
                let response = transport
                    .request("list_project_skills", json!({"directory": project.path,"telemetry":self.store.preferences()?.telemetry}))
                    .await;
                if session_guard.is_none() {
                    transport.shutdown().await;
                }
                let response = response?;
                serde_json::from_value(response)
                    .map_err(|source| Error::InvalidSkillCatalog(source.into()))
            }
        }
    }

    async fn project_skill_transport(
        &self,
        project_id: &str,
    ) -> Result<Option<(Arc<RemoteTransport>, OwnedMutexGuard<()>)>> {
        let project_sessions = self.store.sessions()?;
        for session in project_sessions
            .iter()
            // Discovery must not delay an active turn's approvals or cancellation.
            .filter(|session| {
                session.project_id.as_deref() == Some(project_id)
                    && session.status == SessionStatus::Idle
            })
        {
            let Ok(guard) = self.session_gate(&session.id).await.try_lock_owned() else {
                continue;
            };
            // Sending and model changes use this gate too; recheck after acquiring it.
            let current = match self.store.session(&session.id) {
                Ok(current) => current,
                Err(crate::StorageError::SessionNotFound { .. }) => continue,
                Err(error) => return Err(error.into()),
            };
            if current.status != SessionStatus::Idle {
                continue;
            }
            let live_sessions = self.sessions.lock().await;
            let Some(live) = live_sessions.get(&session.id) else {
                continue;
            };
            if live.event_task.is_finished() || live.completion.has_failed() {
                continue;
            }
            if let AgentHandle::Remote { transport, .. } = &live.handle
                && !transport.is_closed()
            {
                return Ok(Some((Arc::clone(transport), guard)));
            }
        }
        Ok(None)
    }
}
