use std::sync::{Arc, Weak};

use anyhow::{Context, Result, bail};
use copilot_runtime::{SessionMode, SessionOptions};
use serde_json::json;
use tandem_remote::RemoteTransport;
use tokio::{sync::Mutex, task::JoinHandle};
use uuid::Uuid;

use crate::{
    App,
    agent::{AgentEvents, AgentHandle, LiveSession, TurnCompletion},
    app::now,
    events::EventOutcome,
    model::{
        Connection, Message, MessageRole, PermissionMode, Session, SessionStatus, TitleState,
        validate_name,
    },
};

struct Submission<'a> {
    app: &'a App,
    session_id: &'a str,
    settled: bool,
    completion: Option<TurnCompletion>,
}

impl Drop for Submission<'_> {
    fn drop(&mut self) {
        if !self.settled {
            if let Some(completion) = &self.completion {
                completion.fail();
            }
            self.app.fail_session(self.session_id, "发送已取消，请重试");
        }
    }
}

impl App {
    pub(crate) async fn session_gate(&self, id: &str) -> Arc<Mutex<()>> {
        let mut gates = self.session_gates.lock().await;
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(id).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(Mutex::new(()));
        gates.insert(id.to_owned(), Arc::downgrade(&gate));
        gate
    }

    pub(crate) async fn create_session(
        &self,
        project_id: Option<String>,
        provider_id: String,
        model_id: String,
        permission_mode: PermissionMode,
    ) -> Result<Session> {
        if project_id.is_none() && permission_mode != PermissionMode::Ask {
            bail!("聊天不支持自动批准权限，请创建项目工作");
        }
        let _guard = self.mutation_gate.lock().await;
        let provider = self.store.provider(&provider_id)?;
        if !provider.models.iter().any(|model| model.id == model_id) {
            bail!("该供应商未配置选中的模型");
        }
        if let Some(id) = &project_id {
            self.store.project(id)?;
        }
        let session = Session {
            title_state: TitleState::Pending,
            id: Uuid::new_v4().to_string(),
            title: if project_id.is_some() {
                "新工作"
            } else {
                "新对话"
            }
            .into(),
            project_id,
            provider_id,
            model_id,
            permission_mode,
            created_at: now(),
            updated_at: now(),
            status: SessionStatus::Idle,
        };
        self.store.save_session(&session)?;
        Ok(session)
    }

    pub(crate) async fn set_permission_mode(
        &self,
        id: &str,
        permission_mode: PermissionMode,
    ) -> Result<Session> {
        self.editable_work_session(id)?;
        let gate = self.session_gate(id).await;
        let _session_guard = gate
            .try_lock()
            .context("工作正在初始化或处理操作，请稍后切换权限模式")?;
        let updated = {
            let _mutation_guard = self.mutation_gate.lock().await;
            let session = self.editable_work_session(id)?;
            if session.permission_mode == permission_mode {
                return Ok(session);
            }
            let sessions = self.sessions.lock().await;
            // Invalidate before saving without an await in between. If the request
            // is cancelled during disconnect, the old policy can never be reused.
            if let Some(live) = sessions.get(id) {
                live.completion.fail();
            }
            self.store.update_session(id, |session| {
                session.permission_mode = permission_mode;
                session.updated_at = now();
            })?
        };
        self.disconnect_session(id).await;
        Ok(updated)
    }

    fn editable_work_session(&self, id: &str) -> Result<Session> {
        let session = self.store.session(id)?;
        if session.project_id.is_none() {
            bail!("只有项目工作可以切换权限模式");
        }
        if session.status == SessionStatus::Running || self.turns.has_pending_permissions(id)? {
            bail!("请等待工作结束或停止工作后再切换权限模式");
        }
        Ok(session)
    }

    pub(crate) async fn send_message(
        self: &Arc<Self>,
        session_id: &str,
        content: String,
        message_id: Option<Uuid>,
    ) -> Result<()> {
        if content.trim().is_empty() || content.len() > 500_000 {
            bail!("消息不能为空，且不能超过 500 KB");
        }
        let gate = self.session_gate(session_id).await;
        let _session_guard = gate.lock().await;
        // Do not queue a second prompt behind an active turn. Once idle is visible,
        // wait for its forwarding task to finish before marking the next turn running.
        if self.store.session(session_id)?.status == SessionStatus::Running {
            bail!("该对话正在运行，请等待完成或点击停止");
        }
        let previous = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .map(|live| live.completion.clone());
        if let Some(completion) = previous {
            completion.wait().await?;
        }
        let (session, first_turn) = {
            let _mutation_guard = self.mutation_gate.lock().await;
            let session = self.store.session(session_id)?;
            if session.status == SessionStatus::Running {
                bail!("该对话正在运行，请等待完成或点击停止");
            }
            self.titles.mark_interrupted(session_id, false)?;
            let first_turn = !self.store.has_messages(session_id)?;
            let session = self.store.update_session(session_id, |session| {
                session.status = SessionStatus::Running;
                session.updated_at = now();
            })?;
            (session, first_turn)
        };
        self.emit(session_id, "status", json!({"status":"running"}));
        let mut submission = Submission {
            app: self,
            session_id,
            settled: false,
            completion: None,
        };
        let result = self
            .submit_turn(
                &session,
                first_turn,
                content,
                message_id.unwrap_or_else(Uuid::new_v4),
                &mut submission,
            )
            .await;
        if let Err(error) = result {
            if let Some(completion) = &submission.completion {
                completion.fail();
            }
            self.disconnect_session(session_id).await;
            self.fail_session(session_id, &format!("发送失败：{error}"));
            submission.settled = true;
            return Err(error);
        }
        submission.settled = true;
        Ok(())
    }

    async fn submit_turn(
        self: &Arc<Self>,
        session: &Session,
        first_turn: bool,
        content: String,
        message_id: Uuid,
        submission: &mut Submission<'_>,
    ) -> Result<()> {
        let handle = self.ensure_session(session, !first_turn).await?;
        let completion = self
            .sessions
            .lock()
            .await
            .get(&session.id)
            .context("会话连接已失效")?
            .completion
            .clone();
        completion.begin().await?;
        submission.completion = Some(completion);
        let message = Message {
            id: message_id.to_string(),
            session_id: session.id.clone(),
            role: MessageRole::User,
            content: content.clone(),
            created_at: now(),
            interrupted: false,
        };
        self.store.append_message(&message)?;
        self.store.update_session(&session.id, |current| {
            current.updated_at = now();
            current.status = SessionStatus::Running;
        })?;
        self.emit(&session.id, "message", serde_json::to_value(message)?);
        self.emit(&session.id, "status", json!({"status":"running"}));
        handle.send(&content).await
    }

    async fn cached_session(&self, id: &str) -> Option<AgentHandle> {
        let stale = {
            let mut sessions = self.sessions.lock().await;
            let live = sessions.get(id)?;
            if live.handle.is_connected()
                && !live.event_task.is_finished()
                && !live.completion.has_failed()
            {
                return Some(live.handle.clone());
            }
            sessions.remove(id)
        };
        if let Some(mut live) = stale {
            live.stop_forwarding().await;
            if let Err(error) = live.handle.disconnect().await {
                tracing::warn!(%error, "could not close stale agent session");
            }
        }
        None
    }

    async fn ensure_session(
        self: &Arc<Self>,
        session: &Session,
        resume: bool,
    ) -> Result<AgentHandle> {
        let cached = self.cached_session(&session.id).await;
        if let Some(AgentHandle::Local(agent)) = &cached {
            return Ok(AgentHandle::Local(agent.clone()));
        }
        let project = session
            .project_id
            .as_ref()
            .map(|id| self.store.project(id))
            .transpose()?;
        let options = self.session_options(
            session,
            project.as_ref().map(|project| project.path.clone()),
        )?;
        if let Some(AgentHandle::Remote {
            session_id,
            transport,
        }) = cached
        {
            self.start_remote_agent(&transport, &options, true).await?;
            return Ok(AgentHandle::Remote {
                session_id,
                transport,
            });
        }
        let handle = if let Some(project) = project
            && matches!(project.connection, Connection::Ssh { .. })
        {
            let transport = Arc::new(self.remote_client(&project.connection)?.connect().await?);
            AgentHandle::Remote {
                session_id: session.id.clone(),
                transport,
            }
        } else {
            let runtime = self.ensure_runtime().await?;
            let agent = if resume {
                runtime.resume_session(&session.id, &options).await?
            } else {
                runtime.create_session(options.clone()).await?
            };
            AgentHandle::Local(Arc::new(agent))
        };
        // Subscribe before starting the remote agent so its earliest events are retained.
        let incoming = handle.subscribe();
        if let AgentHandle::Remote { transport, .. } = &handle {
            self.start_remote_agent(transport, &options, resume).await?;
        }
        let completion = TurnCompletion::new();
        let event_task = self.forward_events(session.id.clone(), incoming, completion.clone());
        self.sessions.lock().await.insert(
            session.id.clone(),
            LiveSession {
                handle: handle.clone(),
                event_task,
                completion,
            },
        );
        Ok(handle)
    }

    fn forward_events(
        self: &Arc<Self>,
        session_id: String,
        mut incoming: AgentEvents,
        completion: TurnCompletion,
    ) -> JoinHandle<()> {
        let app = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let event = incoming.recv().await;
                let Some(app) = app.upgrade() else { break };
                let keep_forwarding = completion.process_event(|| match event {
                    Ok(event) => app.receive_event(&event),
                    Err(error) => {
                        app.fail_session(&session_id, &format!("{error:#}"));
                        EventOutcome::SessionFailed
                    }
                });
                if !keep_forwarding {
                    break;
                }
            }
        })
    }

    async fn start_remote_agent(
        &self,
        transport: &RemoteTransport,
        options: &SessionOptions,
        resume: bool,
    ) -> Result<()> {
        transport
            .request(
                "start_agent",
                json!({"options":options,"resume":resume,"telemetry":self.store.preferences()?.telemetry}),
            )
            .await?;
        Ok(())
    }

    fn session_options(
        &self,
        session: &Session,
        directory: Option<String>,
    ) -> Result<SessionOptions> {
        let (provider, model) = self.model_settings(&session.provider_id, &session.model_id)?;
        Ok(SessionOptions {
            session_id: Some(session.id.clone()),
            permission_mode: session.permission_mode,
            mode: directory.map_or(SessionMode::Chat, |directory| SessionMode::Work {
                directory: directory.into(),
            }),
            provider,
            model,
            system_prompt: Some("Respond in the user's language. Be precise about what you actually did and verified.".into()),
        })
    }

    pub(crate) async fn abort_session(&self, id: &str) -> Result<()> {
        let gate = self.session_gate(id).await;
        let _session_guard = gate.lock().await;
        self.store.session(id)?;
        self.titles.mark_interrupted(id, true)?;
        let handle = self
            .sessions
            .lock()
            .await
            .get(id)
            .map(|live| (live.handle.clone(), live.completion.clone()));
        if let Some((handle, completion)) = handle {
            handle.abort().await?;
            completion.wait().await?;
        } else {
            self.store.session(id)?;
        }
        self.set_status(id, SessionStatus::Idle)?;
        self.emit(id, "status", json!({"status":"idle"}));
        Ok(())
    }

    pub(crate) async fn respond_permission(
        &self,
        id: &str,
        request_id: &str,
        approved: bool,
    ) -> Result<()> {
        let handle = self
            .sessions
            .lock()
            .await
            .get(id)
            .map(|live| live.handle.clone())
            .context("会话已断开，该授权请求已失效")?;
        handle.approve(request_id, approved).await?;
        self.turns.resolve_permission(id, request_id)
    }

    pub(crate) async fn disconnect_session(&self, id: &str) {
        let live = self.sessions.lock().await.remove(id);
        if let Some(mut live) = live {
            live.stop_forwarding().await;
            if let Err(error) = live.handle.disconnect().await {
                tracing::warn!(%error, "could not disconnect agent session");
            }
        }
        if let Err(error) = self.persist_interrupted(id) {
            tracing::error!(%error, "could not save interrupted output on disconnect");
        }
    }

    pub(crate) async fn delete_session(&self, id: &str) -> Result<()> {
        let _guard = self.mutation_gate.lock().await;
        if self.store.session(id)?.status == SessionStatus::Running {
            bail!("请先停止对话");
        }
        self.disconnect_session(id).await;
        let mut titles = self.titles.lock()?;
        self.store.delete_session(id)?;
        titles.cancel(id);
        Ok(())
    }

    pub(crate) async fn rename_session(&self, id: &str, title: &str) -> Result<()> {
        validate_name(title, "对话名称")?;
        let _guard = self.mutation_gate.lock().await;
        let mut titles = self.titles.lock()?;
        self.store.update_session(id, |session| {
            session.title_state = TitleState::Preserved;
            session.title = title.trim().into();
            session.updated_at = now();
        })?;
        titles.cancel(id);
        self.emit(id, "title", json!({"title":title.trim()}));
        Ok(())
    }

    pub(crate) async fn shutdown_sessions(&self) {
        let ids: Vec<String> = self.sessions.lock().await.keys().cloned().collect();
        for id in ids {
            self.disconnect_session(&id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use copilot_runtime::{EventKind, RuntimeEvent};
    use tempfile::TempDir;

    use super::*;
    use crate::{AppOptions, MemoryVault, Request, model::Project};

    fn application() -> Result<(TempDir, Arc<App>)> {
        let directory = TempDir::new()?;
        let app = App::open(AppOptions {
            data_directory: directory.path().into(),
            secrets: Arc::new(MemoryVault::default()),
            remote_artifacts: tandem_remote::ArtifactLocator::default(),
        })?;
        Ok((directory, app))
    }

    async fn session(app: &App) -> Result<Session> {
        let provider = app.save_provider(
            serde_json::from_value(json!({
                "id":"", "name":"Fixture", "protocol":"openai-completions",
                "baseUrl":"http://127.0.0.1:9999/v1", "models":[{"id":"fixture", "name":"Fixture"}]
            }))?,
            None,
        ).await?;
        app.create_session(None, provider.id, "fixture".into(), PermissionMode::Ask)
            .await
    }

    async fn project(app: &App) -> Result<Project> {
        app.create_project(
            "Fixture work".into(),
            app.options.data_directory.to_string_lossy().into_owned(),
            Connection::Local,
        )
        .await
    }

    async fn work_session(app: &App) -> Result<Session> {
        let chat = session(app).await?;
        let project = project(app).await?;
        app.create_session(
            Some(project.id),
            chat.provider_id,
            chat.model_id,
            PermissionMode::Ask,
        )
        .await
    }

    #[tokio::test]
    async fn session_commands_default_to_ask_and_restrict_permission_modes_to_work() -> Result<()> {
        let (_directory, app) = application()?;
        let chat = session(&app).await?;
        let project = project(&app).await?;
        for project_id in [None, Some(project.id)] {
            for mode in [None, Some("ask"), Some("allow-all")] {
                let mut payload = json!({
                    "projectId":project_id, "providerId":chat.provider_id,
                    "modelId":chat.model_id,
                });
                if let Some(mode) = mode {
                    payload["permissionMode"] = json!(mode);
                }
                let request: Request = serde_json::from_value(json!({
                    "command":"create_session", "payload":payload,
                }))?;
                let result = app.dispatch(request).await;
                if project_id.is_none() && mode == Some("allow-all") {
                    assert!(result.is_err());
                } else {
                    assert_eq!(result?["permissionMode"], mode.unwrap_or("ask"));
                }
            }
        }
        for mode in [PermissionMode::Ask, PermissionMode::AllowAll] {
            assert!(app.set_permission_mode(&chat.id, mode).await.is_err());
        }
        assert_eq!(
            app.store.session(&chat.id)?.permission_mode,
            PermissionMode::Ask
        );
        Ok(())
    }

    #[tokio::test]
    async fn permission_changes_do_not_wait_for_running_initializing_or_pending_work() -> Result<()>
    {
        for (status, pending_approval, initializing) in [
            (SessionStatus::Running, false, false),
            (SessionStatus::Idle, true, false),
            (SessionStatus::Idle, false, true),
            (SessionStatus::Error, true, false),
            (SessionStatus::Error, false, true),
        ] {
            let (_directory, app) = application()?;
            let work = work_session(&app).await?;
            app.set_status(&work.id, status)?;
            if pending_approval {
                app.turns
                    .permission(&work.id, &json!({"requestId":"pending"}))?;
            }
            let gate = app.session_gate(&work.id).await;
            let _initializing = initializing.then(|| gate.try_lock()).transpose()?;
            // A rejected request must finish even while configuration changes are blocked.
            let _mutation_guard = app.mutation_gate.lock().await;
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                app.set_permission_mode(&work.id, PermissionMode::AllowAll),
            )
            .await?;
            assert!(
                result.is_err(),
                "{status:?}/{pending_approval}/{initializing}"
            );
            let stored = app.store.session(&work.id)?;
            assert_eq!(stored.permission_mode, PermissionMode::Ask);
            assert_eq!(stored.status, status);
        }
        Ok(())
    }

    #[tokio::test]
    async fn failed_work_can_revoke_full_permission_without_losing_history() -> Result<()> {
        let (_directory, app) = application()?;
        let work = work_session(&app).await?;
        app.set_permission_mode(&work.id, PermissionMode::AllowAll)
            .await?;
        app.store.append_message(&Message {
            id: Uuid::new_v4().to_string(),
            session_id: work.id.clone(),
            role: MessageRole::Assistant,
            content: "Partial output before the failure".into(),
            created_at: now(),
            interrupted: true,
        })?;
        app.set_status(&work.id, SessionStatus::Error)?;
        let history = serde_json::to_value(app.store.messages(&work.id)?)?;
        let updated = app
            .set_permission_mode(&work.id, PermissionMode::Ask)
            .await?;
        assert_eq!(updated.permission_mode, PermissionMode::Ask);
        assert_eq!(updated.status, SessionStatus::Error);
        assert_eq!(
            app.store.session(&work.id)?.permission_mode,
            PermissionMode::Ask
        );
        assert_eq!(
            serde_json::to_value(app.store.messages(&work.id)?)?,
            history
        );
        Ok(())
    }

    #[tokio::test]
    async fn permission_changes_preserve_history_and_reach_resumed_local_and_remote_options()
    -> Result<()> {
        let (directory, mut app) = application()?;
        let work = work_session(&app).await?;
        app.rename_session(&work.id, "Keep this title").await?;
        for (role, content) in [
            (MessageRole::User, "Remember this request"),
            (MessageRole::Assistant, "Remember this answer"),
        ] {
            app.store.append_message(&Message {
                id: Uuid::new_v4().to_string(),
                session_id: work.id.clone(),
                role,
                content: content.into(),
                created_at: now(),
                interrupted: false,
            })?;
        }
        let history = serde_json::to_value(app.store.messages(&work.id)?)?;
        for mode in [PermissionMode::AllowAll, PermissionMode::Ask] {
            let request: Request = serde_json::from_value(json!({
                "command":"set_permission_mode",
                "payload":{"sessionId":work.id,"permissionMode":mode},
            }))?;
            let updated: Session = serde_json::from_value(app.dispatch(request).await?)?;
            assert_eq!(updated.permission_mode, mode);
            assert_eq!(updated.title, "Keep this title");
            app = App::open(AppOptions {
                data_directory: directory.path().into(),
                secrets: Arc::new(MemoryVault::default()),
                remote_artifacts: tandem_remote::ArtifactLocator::default(),
            })?;
            let stored = app.store.session(&work.id)?;
            assert_eq!(stored.permission_mode, mode);
            assert_eq!(stored.status, SessionStatus::Idle);
            assert!(app.store.has_messages(&work.id)?);
            assert_eq!(
                serde_json::to_value(app.store.messages(&work.id)?)?,
                history
            );
            let project = app
                .store
                .project(stored.project_id.as_deref().context("missing project")?)?;
            let options = app.session_options(&stored, Some(project.path))?;
            assert_eq!(options.session_id.as_deref(), Some(work.id.as_str()));
            assert_eq!(options.permission_mode, mode);
            assert!(matches!(options.mode, SessionMode::Work { .. }));
            // SSH sends this exact SessionOptions object to start_agent.
            let remote = serde_json::to_value(&options)?;
            let restored: SessionOptions = serde_json::from_value(remote)?;
            assert_eq!(restored.permission_mode, mode);
        }
        Ok(())
    }

    #[tokio::test]
    async fn same_session_operations_serialize_without_locking_other_sessions() -> Result<()> {
        let (_directory, app) = application()?;
        let first = session(&app).await?;
        let second = session(&app).await?;
        let gate = app.session_gate(&first.id).await;
        let blocked = gate.lock().await;
        let global = app.mutation_gate.lock().await;
        let waiting = tokio::spawn({
            let app = Arc::clone(&app);
            async move { app.abort_session(&first.id).await }
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            app.abort_session(&second.id),
        )
        .await??;
        assert!(!waiting.is_finished());
        drop(global);
        drop(blocked);
        tokio::time::timeout(std::time::Duration::from_secs(1), waiting).await???;
        Ok(())
    }

    #[tokio::test]
    async fn background_status_updates_cannot_overwrite_renamed_titles() -> Result<()> {
        let (_directory, app) = application()?;
        let session = session(&app).await?;
        std::thread::scope(|scope| -> Result<()> {
            let statuses = scope.spawn(|| -> Result<()> {
                for _ in 0..250 {
                    app.set_status(&session.id, SessionStatus::Running)?;
                    app.set_status(&session.id, SessionStatus::Idle)?;
                }
                Ok(())
            });
            for index in 0..250 {
                app.store.update_session(&session.id, |session| {
                    session.title = format!("Renamed {index}");
                })?;
            }
            statuses
                .join()
                .map_err(|_| anyhow::anyhow!("status worker panicked"))??;
            Ok(())
        })?;
        let stored = app.store.session(&session.id)?;
        assert_eq!(stored.title, "Renamed 249");
        assert_eq!(stored.status, SessionStatus::Idle);
        Ok(())
    }

    #[tokio::test]
    async fn unfinished_output_survives_idle_without_duplicating_completed_messages() -> Result<()>
    {
        for completed in [true, false] {
            let (_directory, app) = application()?;
            let session = session(&app).await?;
            let receive = |kind, data| {
                app.receive_event(&RuntimeEvent {
                    session_id: session.id.clone(),
                    kind,
                    data,
                });
            };
            receive(
                EventKind::Delta,
                json!({"messageId":"message-1","content":"Visible output"}),
            );
            receive(
                EventKind::Permission,
                json!({"requestId":"approval-1","kind":"write","description":"Write file"}),
            );
            let snapshot = app.snapshot().await?;
            assert_eq!(snapshot.active_turns[&session.id].stream, "Visible output");
            assert_eq!(snapshot.active_turns[&session.id].permissions.len(), 1);
            if completed {
                receive(
                    EventKind::Message,
                    json!({"id":"message-1","content":"Visible output"}),
                );
            }
            receive(EventKind::Status, json!({"state":"idle"}));
            receive(EventKind::Status, json!({"state":"idle"}));
            let messages = app.store.messages(&session.id)?;
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].content, "Visible output");
            assert_eq!(messages[0].interrupted, !completed);
            assert!(app.snapshot().await?.active_turns.is_empty());
        }
        Ok(())
    }

    #[tokio::test]
    async fn completion_barrier_drains_output_and_discards_events_after_failure() -> Result<()> {
        for (completed, terminal_kind, terminal_data) in [
            (true, EventKind::Status, json!({"state":"idle"})),
            (false, EventKind::Error, json!({"message":"stopped"})),
            (false, EventKind::Status, json!({"state":"invalid"})),
        ] {
            let (_directory, app) = application()?;
            let session = session(&app).await?;
            let completion = TurnCompletion::new();
            completion.begin().await?;
            let (outgoing, incoming) = tokio::sync::broadcast::channel(16);
            let event = |kind, data| RuntimeEvent {
                session_id: session.id.clone(),
                kind,
                data,
            };
            outgoing.send(event(
                EventKind::Delta,
                json!({"messageId":"message-1","content":"reply"}),
            ))?;
            if completed {
                outgoing.send(event(
                    EventKind::Message,
                    json!({"id":"message-1","content":"reply"}),
                ))?;
            }
            outgoing.send(event(terminal_kind, terminal_data))?;
            if !completed {
                // A trailing idle from a failed SDK turn must not overwrite its error.
                outgoing.send(event(EventKind::Status, json!({"state":"idle"})))?;
            }
            // The terminal event is already queued, but the application has not persisted it.
            assert!(app.store.messages(&session.id)?.is_empty());
            let mut event_task = app.forward_events(
                session.id.clone(),
                AgentEvents::Local(incoming),
                completion.clone(),
            );
            completion.wait().await?;
            let messages = app.store.messages(&session.id)?;
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].content, "reply");
            assert_eq!(messages[0].interrupted, !completed);
            assert!(app.turns.snapshot()?.is_empty());
            if completed {
                assert_eq!(app.store.session(&session.id)?.status, SessionStatus::Idle);
                event_task.abort();
                let _ = (&mut event_task).await;
            } else {
                tokio::time::timeout(std::time::Duration::from_secs(1), event_task).await??;
                assert_eq!(app.store.session(&session.id)?.status, SessionStatus::Error);
                assert!(completion.has_failed());
                assert!(completion.begin().await.is_err());
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_submission_preserves_its_error_when_old_idle_is_queued() -> Result<()> {
        let (_directory, app) = application()?;
        let session = session(&app).await?;
        app.set_status(&session.id, SessionStatus::Running)?;
        let completion = TurnCompletion::new();
        completion.begin().await?;
        assert!(
            completion.process_event(|| app.receive_event(&RuntimeEvent {
                session_id: session.id.clone(),
                kind: EventKind::Delta,
                data: json!({"messageId":"message-1","content":"partial reply"}),
            }))
        );
        let (outgoing, incoming) = tokio::sync::broadcast::channel(4);
        outgoing.send(RuntimeEvent {
            session_id: session.id.clone(),
            kind: EventKind::Status,
            data: json!({"state":"idle"}),
        })?;
        let (started, ready) = tokio::sync::oneshot::channel();
        let sending = tokio::spawn({
            let app = Arc::clone(&app);
            let session_id = session.id.clone();
            let completion = completion.clone();
            async move {
                let _submission = Submission {
                    app: &app,
                    session_id: &session_id,
                    settled: false,
                    completion: Some(completion),
                };
                let _ = started.send(());
                std::future::pending::<()>().await;
            }
        });
        ready.await?;
        sending.abort();
        let _ = sending.await;
        let event_task = app.forward_events(
            session.id.clone(),
            AgentEvents::Local(incoming),
            completion.clone(),
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), event_task).await??;
        assert!(completion.has_failed());
        assert_eq!(app.store.session(&session.id)?.status, SessionStatus::Error);
        let messages = app.store.messages(&session.id)?;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "partial reply");
        assert!(messages[0].interrupted);
        assert!(app.turns.snapshot()?.is_empty());
        Ok(())
    }
}
