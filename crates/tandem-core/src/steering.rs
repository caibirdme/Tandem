//! Explicit corrections reuse the live session without restarting its current turn.

use std::sync::Arc;

use copilot_runtime::{
    MessageAttachment, SkillSelection, validate_attachment_protocol, validate_attachments,
};
use uuid::Uuid;

use crate::{
    App, Error, Result,
    app::now,
    model::{AttachmentMetadata, Message, MessageRole, SessionStatus},
};

impl App {
    pub(crate) async fn steer_message(
        self: &Arc<Self>,
        session_id: &str,
        content: String,
        message_id: Uuid,
        selected_skills: &[SkillSelection],
        attachments: &[MessageAttachment],
    ) -> Result<()> {
        if (content.trim().is_empty() && attachments.is_empty()) || content.len() > 500_000 {
            return Err(Error::InvalidMessage);
        }
        let bytes = validate_attachments(attachments)?;
        let gate = self.session_gate(session_id).await;
        let guard = gate.try_lock().map_err(|_| Error::ConversationRunning)?;
        let session = self.store.session(session_id)?;
        if self
            .store
            .has_message(session_id, &message_id.to_string())?
        {
            return Err(Error::SteeringAlreadyRecorded);
        }
        if !selected_skills.is_empty() && session.project_id.is_none() {
            return Err(Error::ChatSkillSelection);
        }
        let provider = self.store.provider(&session.provider_id)?;
        for attachment in attachments {
            validate_attachment_protocol(
                &attachment.name,
                &attachment.mime_type,
                provider.protocol.into(),
            )?;
        }
        // A click can race the final idle notification. Idle sends still use the
        // regular turn initializer and its review baseline, not an invented busy turn.
        if session.status != SessionStatus::Running {
            drop(guard);
            let result = self
                .send_message(
                    session_id,
                    content,
                    message_id,
                    selected_skills,
                    attachments,
                    false,
                )
                .await;
            return match result {
                Err(error)
                    if self
                        .store
                        .has_message(session_id, &message_id.to_string())? =>
                {
                    Err(Error::SteeringDeliveryUnknown(Box::new(error)))
                }
                result => result,
            };
        }
        let (handle, completion) = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .filter(|live| live.handle.is_connected() && !live.completion.has_failed())
            .map(|live| (live.handle.clone(), live.completion.clone()))
            .ok_or(Error::SessionDisconnected)?;
        let prepared = handle
            .prepare_steering(&content, selected_skills, attachments)
            .await?;
        let starts_new_turn = prepared.starts_new_turn();
        if starts_new_turn {
            // Runtime admission has reserved the new turn and withholds later
            // idle events. Drain the preceding terminal event before replacing
            // its capture and completion state.
            completion.wait().await?;
            self.wait_review_capture(session_id).await;
            self.begin_review_turn(&session, &message_id.to_string(), &handle)
                .await;
            completion.begin().await?;
        }
        let message = steering_message(
            session_id,
            content,
            message_id,
            selected_skills,
            attachments,
            &bytes,
        );
        self.store
            .update_session(session_id, |current| current.updated_at = now())?;
        if starts_new_turn {
            self.set_status(session_id, SessionStatus::Running)?;
        }
        let message = self
            .store
            .append_message_with_attachments(&message, &bytes)?;
        if starts_new_turn {
            self.commit_review_turn(session_id, &message_id.to_string())
                .await;
            self.emit(
                session_id,
                "status",
                serde_json::json!({"status":"running"}),
            );
        }
        self.emit(session_id, "message", serde_json::to_value(message)?);
        // Once committed, a failed/abandoned transport acknowledgement cannot be
        // retried safely. Crucially, it must not abort or mark the active turn failed.
        handle
            .send(prepared)
            .await
            .map_err(|error| Error::SteeringDeliveryUnknown(Box::new(error)))?;
        Ok(())
    }
}

fn steering_message(
    session_id: &str,
    content: String,
    message_id: Uuid,
    selected_skills: &[SkillSelection],
    attachments: &[MessageAttachment],
    bytes: &[Vec<u8>],
) -> Message {
    Message {
        id: message_id.to_string(),
        session_id: session_id.into(),
        role: MessageRole::User,
        content,
        selected_skills: selected_skills.to_vec(),
        attachments: attachments
            .iter()
            .zip(bytes)
            .map(|(attachment, data)| AttachmentMetadata {
                id: Uuid::new_v4().to_string(),
                name: attachment.name.clone(),
                mime_type: attachment.mime_type.clone(),
                size: data.len(),
            })
            .collect(),
        created_at: now(),
        sequence: None,
        activity: None,
        reasoning: None,
        agent_id: None,
        agent: None,
        fleet: false,
        interrupted: false,
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::AppOptions;

    async fn application() -> Result<(TempDir, Arc<App>, String)> {
        let directory = TempDir::new()?;
        let app = App::open(AppOptions::local(directory.path().into()))?;
        let provider = app.save_provider(serde_json::from_value(json!({
            "id":"", "name":"Fixture", "protocol":"anthropic", "baseUrl":"https://example.invalid",
            "models":[{"id":"fixture","name":"Fixture","contextWindow":200_000}]
        }))?, None).await?;
        let session = app
            .create_session(None, provider.id, "fixture".into(), None, None)
            .await?;
        app.set_status(&session.id, SessionStatus::Running)?;
        Ok((directory, app, session.id))
    }

    #[tokio::test]
    async fn rejected_steering_does_not_fail_or_change_the_running_conversation() -> Result<()> {
        let (_directory, app, id) = application().await?;
        for content in [String::new(), "Valid correction".into()] {
            let error = app
                .steer_message(&id, content, Uuid::new_v4(), &[], &[])
                .await
                .err();
            assert!(matches!(
                error,
                Some(Error::InvalidMessage | Error::SessionDisconnected)
            ));
            assert_eq!(app.store.session(&id)?.status, SessionStatus::Running);
            assert!(app.messages(&id)?.is_empty());
        }
        Ok(())
    }

    #[tokio::test]
    async fn chat_rejects_work_skills_without_recording_or_interrupting() -> Result<()> {
        let (_directory, app, id) = application().await?;
        let selected = [SkillSelection {
            name: "Review".into(),
            path: "skills/review/SKILL.md".into(),
        }];
        let result = app
            .steer_message(&id, "Use this skill".into(), Uuid::new_v4(), &selected, &[])
            .await;
        assert!(matches!(result, Err(Error::ChatSkillSelection)));
        assert!(app.messages(&id)?.is_empty());
        assert_eq!(app.store.session(&id)?.status, SessionStatus::Running);
        Ok(())
    }

    #[tokio::test]
    async fn pending_runtime_work_keeps_ordinary_sends_and_model_edits_blocked() -> Result<()> {
        let (_directory, app, id) = application().await?;
        let session = app.store.session(&id)?;
        // The runtime withholds intermediate idle while an accepted correction
        // is waiting for consumption. No second initializer may take its place.
        let send = app
            .send_message(&id, "Another turn".into(), Uuid::new_v4(), &[], &[], false)
            .await;
        assert!(matches!(send, Err(Error::ConversationRunning)));
        let change = app
            .set_session_model(&id, session.provider_id, session.model_id)
            .await;
        assert!(matches!(change, Err(Error::ModelChangeRunning)));
        assert_eq!(app.store.session(&id)?.status, SessionStatus::Running);
        assert!(app.messages(&id)?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn recorded_steering_is_never_sent_twice_even_without_a_live_runtime() -> Result<()> {
        let (_directory, app, id) = application().await?;
        let message_id = Uuid::new_v4();
        app.store.append_message(&Message {
            id: message_id.to_string(),
            session_id: id.clone(),
            role: MessageRole::User,
            content: "Correction".into(),
            selected_skills: Vec::new(),
            attachments: Vec::new(),
            created_at: now(),
            sequence: None,
            activity: None,
            reasoning: None,
            agent_id: None,
            agent: None,
            fleet: false,
            interrupted: false,
        })?;
        let error = app
            .steer_message(&id, "Correction".into(), message_id, &[], &[])
            .await
            .err();
        assert!(matches!(error, Some(Error::SteeringAlreadyRecorded)));
        assert_eq!(
            error.map(|error| error.code()),
            Some(crate::ErrorCode::DeliveryUnknown)
        );
        assert_eq!(app.messages(&id)?.len(), 1);
        assert_eq!(app.store.session(&id)?.status, SessionStatus::Running);
        Ok(())
    }
}
