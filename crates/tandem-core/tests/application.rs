use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tandem_core::{
    App, AppOptions, MemoryVault, Request, SecretError, SecretStore, SendMessageRequest,
    model::Provider,
};
use tandem_remote::ArtifactLocator;
use tempfile::TempDir;

fn app(directory: &TempDir, secrets: Arc<dyn SecretStore>) -> Result<Arc<App>> {
    Ok(App::open(AppOptions {
        data_directory: directory.path().to_owned(),
        secrets,
        remote_artifacts: ArtifactLocator::default(),
    })?)
}

async fn call(app: &Arc<App>, command: &str, payload: Value) -> Result<Value> {
    Ok(app
        .dispatch(serde_json::from_value::<Request>(
            json!({"command":command,"payload":payload}),
        )?)
        .await?)
}

fn provider() -> Value {
    json!({"id":"","name":"Test inference","protocol":"openai-completions","baseUrl":"http://127.0.0.1:9999/v1", "models":[{"id":"model-one","name":"Model one","reasoningEffort":null,"contextWindow":32000,"maxOutputTokens":4096}]})
}

#[tokio::test]
async fn application_failures_remain_typed_through_dispatch() -> Result<()> {
    use tandem_core::{Error, ErrorCode, ErrorResponse, StorageError};

    let directory = TempDir::new()?;
    let application = App::open(AppOptions::local(directory.path().into()))?;
    let missing = application
        .dispatch(Request::Messages {
            session_id: "missing".into(),
        })
        .await;
    assert!(
        matches!(&missing, Err(Error::Storage(StorageError::SessionNotFound { id })) if id == "missing")
    );
    let missing = missing.err().context("missing session must fail")?;
    assert_eq!(missing.code(), ErrorCode::NotFound);
    assert_eq!(
        serde_json::to_value(ErrorResponse::from(missing))?["code"],
        "not_found"
    );

    let invalid = application
        .dispatch(Request::SendMessage(SendMessageRequest {
            fleet: false,
            session_id: "missing".into(),
            content: " ".into(),
            message_id: uuid::Uuid::new_v4(),
            selected_skills: Vec::new(),
            attachments: Vec::new(),
        }))
        .await;
    assert!(matches!(invalid, Err(Error::InvalidMessage)));

    let saved = call(
        &application,
        "save_provider",
        json!({"provider":provider()}),
    )
    .await?;
    let session = call(
        &application,
        "create_session",
        json!({
            "projectId":null,"providerId":saved["id"],"modelId":"model-one"
        }),
    )
    .await?;
    let absent_model = application
        .dispatch(Request::SetSessionModel {
            session_id: session["id"].as_str().context("session id")?.into(),
            provider_id: saved["id"].as_str().context("provider id")?.into(),
            model_id: "absent".into(),
        })
        .await;
    assert!(
        matches!(absent_model, Err(Error::ModelNotConfigured { model_id, .. }) if model_id == "absent")
    );

    rusqlite::Connection::open(directory.path().join("tandem.sqlite3"))?.execute(
        "UPDATE sessions SET data=json_set(data,'$.status','running') WHERE id=?1",
        [session["id"].as_str().context("session id")?],
    )?;
    let running = application
        .dispatch(Request::DeleteSession {
            session_id: session["id"].as_str().context("session id")?.into(),
        })
        .await;
    assert!(matches!(running, Err(Error::ConversationRunning)));
    assert_eq!(
        running
            .err()
            .context("running session must block deletion")?
            .code(),
        ErrorCode::Conflict
    );
    Ok(())
}

#[test]
fn send_message_requires_valid_client_identity() -> Result<()> {
    let id = uuid::Uuid::new_v4();
    for (message_id, valid) in [
        (Value::Null, false),
        (json!(id), true),
        (json!("invalid"), false),
    ] {
        let request = serde_json::from_value::<Request>(json!({
            "command":"send_message",
            "payload":{"sessionId":"session", "content":"hello", "messageId":message_id}
        }));
        assert_eq!(request.is_ok(), valid);
        if valid {
            let Request::SendMessage(SendMessageRequest {
                message_id: parsed, ..
            }) = request?
            else {
                anyhow::bail!("expected a send request");
            };
            assert_eq!(parsed, id);
        }
    }
    let missing_id = serde_json::from_value::<Request>(json!({
        "command":"send_message", "payload":{"sessionId":"session", "content":"hello"}
    }));
    assert!(missing_id.is_err());
    Ok(())
}

#[test]
fn selected_skills_require_structured_names_and_paths() -> Result<()> {
    for (selected, valid) in [
        (json!([]), true),
        (
            json!([{"name":"review","path":"/project/.agents/skills/review/SKILL.md"}]),
            true,
        ),
        (json!(["review"]), false),
        (json!([{"name":"review"}]), false),
        (json!([{"name":42,"path":"/project/skill"}]), false),
        (Value::Null, false),
    ] {
        let request = serde_json::from_value::<Request>(json!({
            "command":"send_message",
            "payload":{
                "sessionId":"session", "content":"$review", "messageId":uuid::Uuid::new_v4(),
                "selectedSkills":selected,
            }
        }));
        assert_eq!(request.is_ok(), valid);
        if valid {
            let Request::SendMessage(SendMessageRequest {
                selected_skills, ..
            }) = request?
            else {
                anyhow::bail!("expected a send request");
            };
            assert_eq!(
                selected_skills.len(),
                selected.as_array().context("array")?.len()
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn attachments_are_validated_before_runtime_or_session_changes() -> Result<()> {
    let directory = TempDir::new()?;
    let application = app(&directory, Arc::new(MemoryVault::default()))?;
    let saved = call(
        &application,
        "save_provider",
        json!({"provider":provider()}),
    )
    .await?;
    let session = call(
        &application,
        "create_session",
        json!({
            "projectId":null, "providerId":saved["id"], "modelId":"model-one"
        }),
    )
    .await?;
    let attachment = json!({"name":"example.txt","mimeType":"text/plain","data":"aGVsbG8="});
    for attachments in [
        json!([{"name":"example.png","mimeType":"image/png","data":"!"}]),
        json!(vec![attachment.clone(); 11]),
    ] {
        let request = serde_json::from_value(json!({
            "command":"send_message",
            "payload":{
                "sessionId":session["id"], "content":"", "messageId":uuid::Uuid::new_v4(),
                "attachments":attachments,
            }
        }))?;
        let error = application
            .dispatch(request)
            .await
            .err()
            .context("invalid attachment must fail")?;
        assert!(matches!(error, tandem_core::Error::Attachment(_)));
        assert_eq!(error.code(), tandem_core::ErrorCode::InvalidInput);
    }
    let snapshot = call(&application, "snapshot", json!({})).await?;
    assert_eq!(snapshot["sessions"][0]["status"], "idle");
    assert_eq!(
        call(&application, "messages", json!({"sessionId":session["id"]})).await?,
        json!([])
    );
    assert!(!directory.path().join("runtime").exists());
    let attachment_only = serde_json::from_value(json!({
        "command":"send_message",
        "payload":{
            "sessionId":"missing", "content":"", "messageId":uuid::Uuid::new_v4(),
            "attachments":[attachment],
        }
    }))?;
    let error = application
        .dispatch(attachment_only)
        .await
        .err()
        .context("missing session must fail after attachment-only validation")?;
    assert!(matches!(
        error,
        tandem_core::Error::Storage(tandem_core::StorageError::SessionNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn pdf_send_rejects_completions_before_runtime_or_state_changes() -> Result<()> {
    let directory = TempDir::new()?;
    let application = app(&directory, Arc::new(MemoryVault::default()))?;
    let saved = call(
        &application,
        "save_provider",
        json!({"provider":provider()}),
    )
    .await?;
    let session = call(
        &application,
        "create_session",
        json!({"projectId":null,"providerId":saved["id"],"modelId":"model-one"}),
    )
    .await?;
    for (content, mime_type) in [
        ("Read this PDF", "application/pdf"),
        ("", "application/octet-stream"),
    ] {
        let request = serde_json::from_value(json!({
            "command":"send_message",
            "payload":{
                "sessionId":session["id"], "content":content, "messageId":uuid::Uuid::new_v4(),
                "attachments":[{"name":"REPORT.PDF","mimeType":mime_type,"data":"JVBERi0xLjcK"}],
            }
        }))?;
        let error = application
            .dispatch(request)
            .await
            .err()
            .context("unsupported PDF must fail")?;
        assert!(matches!(
            error,
            tandem_core::Error::Attachment(
                copilot_runtime::AttachmentError::PdfUnsupportedByProvider
            )
        ));
        assert_eq!(error.code(), tandem_core::ErrorCode::InvalidInput);
    }
    assert_eq!(
        call(&application, "messages", json!({"sessionId":session["id"]})).await?,
        json!([])
    );
    let snapshot = call(&application, "snapshot", json!({})).await?;
    assert_eq!(snapshot["sessions"][0], session);
    assert!(!directory.path().join("runtime").exists());
    Ok(())
}

#[tokio::test]
async fn chat_rejects_skill_selection_before_starting_a_runtime_or_saving_a_message() -> Result<()>
{
    let directory = TempDir::new()?;
    let application = app(&directory, Arc::new(MemoryVault::default()))?;
    let saved = call(
        &application,
        "save_provider",
        json!({"provider":provider()}),
    )
    .await?;
    let session = call(
        &application,
        "create_session",
        json!({
            "projectId":null, "providerId":saved["id"], "modelId":"model-one"
        }),
    )
    .await?;
    let request: Request = serde_json::from_value(json!({
        "command":"send_message",
        "payload":{
            "sessionId":session["id"], "content":"$review", "messageId":uuid::Uuid::new_v4(),
            "selectedSkills":[{"name":"review","path":"/project/.agents/skills/review/SKILL.md"}]
        }
    }))?;
    let error = application
        .dispatch(request)
        .await
        .err()
        .context("Chat must reject skills")?;
    assert!(matches!(error, tandem_core::Error::ChatSkillSelection));
    assert_eq!(error.code(), tandem_core::ErrorCode::InvalidInput);
    let messages = call(&application, "messages", json!({"sessionId":session["id"]})).await?;
    assert_eq!(messages, json!([]));
    assert!(!directory.path().join("runtime").exists());
    Ok(())
}

#[tokio::test]
async fn local_credentials_can_be_read_on_demand_and_survive_restart() -> Result<()> {
    let directory = TempDir::new()?;
    let mut application = App::open(AppOptions::local(directory.path().to_owned()))?;
    let saved = call(
        &application,
        "save_provider",
        json!({"provider":provider(),"apiKey":"integration-secret"}),
    )
    .await?;
    let id = saved["id"].as_str().context("provider id")?;
    assert_eq!(saved["hasApiKey"], true);
    assert_eq!(
        call(&application, "get_provider_api_key", json!({"id":id})).await?,
        "integration-secret"
    );
    application = App::open(AppOptions::local(directory.path().to_owned()))?;
    let saved = call(&application, "save_provider", json!({"provider":saved})).await?;
    assert_eq!(saved["hasApiKey"], true);
    assert_eq!(
        call(&application, "get_provider_api_key", json!({"id":id})).await?,
        "integration-secret"
    );
    let snapshot = call(&application, "snapshot", json!({})).await?;
    assert!(!snapshot.to_string().contains("integration-secret"));
    let database = rusqlite::Connection::open(directory.path().join("tandem.sqlite3"))?;
    let persisted: String = database.query_row(
        "SELECT secret FROM provider_secrets WHERE id=?1",
        [id],
        |row| row.get(0),
    )?;
    assert_eq!(persisted, "integration-secret");
    let saved = call(
        &application,
        "save_provider",
        json!({"provider":saved,"apiKey":""}),
    )
    .await?;
    assert_eq!(saved["hasApiKey"], false);
    assert_eq!(
        call(&application, "get_provider_api_key", json!({"id":id})).await?,
        Value::Null
    );
    Ok(())
}

#[tokio::test]
async fn missing_and_keyless_providers_do_not_read_credential_storage() -> Result<()> {
    struct UnreadableVault;
    impl SecretStore for UnreadableVault {
        fn get(&self, _: &str) -> std::result::Result<Option<String>, SecretError> {
            Err(SecretError::LockPoisoned)
        }

        fn set(&self, _: &str, _: Option<&str>) -> std::result::Result<(), SecretError> {
            Ok(())
        }

        fn get_telemetry_api_key(&self) -> std::result::Result<Option<String>, SecretError> {
            Err(SecretError::LockPoisoned)
        }

        fn set_telemetry_api_key(&self, _: Option<&str>) -> std::result::Result<(), SecretError> {
            Err(SecretError::LockPoisoned)
        }
    }
    let directory = TempDir::new()?;
    let application = app(&directory, Arc::new(UnreadableVault))?;
    let unknown = call(
        &application,
        "get_provider_api_key",
        json!({"id":"unknown"}),
    )
    .await;
    assert!(unknown.is_err_and(|error| error.to_string().contains("Provider does not exist")));
    let saved = call(
        &application,
        "save_provider",
        json!({"provider":provider()}),
    )
    .await?;
    assert_eq!(saved["hasApiKey"], false);
    assert_eq!(
        call(
            &application,
            "get_provider_api_key",
            json!({"id":saved["id"]})
        )
        .await?,
        Value::Null
    );
    let updated = call(&application, "save_provider", json!({"provider":saved})).await?;
    assert_eq!(updated["hasApiKey"], false);
    call(&application, "save_preferences", json!({
        "telemetry":{"enabled":true,"endpoint":"http://127.0.0.1:4318","protocol":"http/json","captureContent":false},
        "cliPath":null
    })).await?;
    assert!(
        call(&application, "get_telemetry_api_key", json!({}))
            .await
            .is_err()
    );
    let snapshot = call(&application, "snapshot", json!({})).await?;
    assert_eq!(snapshot["settings"]["telemetry"]["enabled"], true);
    Ok(())
}

#[tokio::test]
async fn projects_and_sessions_survive_restart_and_deletions_respect_references() -> Result<()> {
    let directory = TempDir::new()?;
    let secrets = Arc::new(MemoryVault::default());
    let application = app(&directory, secrets.clone())?;
    let provider = call(
        &application,
        "save_provider",
        json!({"provider":provider()}),
    )
    .await?;
    let project = call(
        &application,
        "create_project",
        json!({"name":"Repository","path":directory.path(),"connection":{"kind":"local"}}),
    )
    .await?;
    let session = call(
        &application,
        "create_session",
        json!({"projectId":project["id"],"providerId":provider["id"],"modelId":"model-one"}),
    )
    .await?;
    assert!(
        call(
            &application,
            "delete_provider",
            json!({"id":provider["id"]})
        )
        .await
        .is_err()
    );
    drop(application);
    let reopened = app(&directory, secrets)?;
    let snapshot = call(&reopened, "snapshot", json!({})).await?;
    assert_eq!(snapshot["sessions"][0]["id"], session["id"]);
    assert_eq!(snapshot["projects"][0]["id"], project["id"]);
    call(&reopened, "delete_project", json!({"id":project["id"]})).await?;
    assert!(
        call(&reopened, "messages", json!({"sessionId":session["id"]}))
            .await
            .is_err()
    );
    call(&reopened, "delete_provider", json!({"id":provider["id"]})).await?;
    Ok(())
}

#[test]
fn provider_validation_rejects_broken_inference_configuration() -> Result<()> {
    let cases = [
        ("valid", "/baseUrl", json!("https://example.com/v1"), true),
        (
            "embedded credentials",
            "/baseUrl",
            json!("https://secret@example.com/v1"),
            false,
        ),
        (
            "file protocol",
            "/baseUrl",
            json!("file:///tmp/model"),
            false,
        ),
        (
            "query credential",
            "/baseUrl",
            json!("https://example.com?key=secret"),
            false,
        ),
        ("no models", "/models", json!([]), false),
        ("zero output", "/models/0/maxOutputTokens", json!(0), false),
        (
            "output exceeds context",
            "/models/0/maxOutputTokens",
            json!(64000),
            false,
        ),
        (
            "invalid reasoning",
            "/models/0/reasoningEffort",
            json!("maximum"),
            false,
        ),
        ("empty model id", "/models/0/id", json!(" "), false),
    ];
    for (name, pointer, replacement, valid) in cases {
        let mut value = provider();
        *value.pointer_mut(pointer).context("test pointer")? = replacement;
        let provider: Provider = serde_json::from_value(value)?;
        assert_eq!(provider.validate().is_ok(), valid, "{name}");
    }
    Ok(())
}

#[tokio::test]
async fn invalid_model_and_file_project_are_rejected_before_runtime_start() -> Result<()> {
    let directory = TempDir::new()?;
    let application = app(&directory, Arc::new(MemoryVault::default()))?;
    let provider = call(
        &application,
        "save_provider",
        json!({"provider":provider()}),
    )
    .await?;
    assert!(
        call(
            &application,
            "create_session",
            json!({"projectId":null,"providerId":provider["id"],"modelId":"missing"})
        )
        .await
        .is_err()
    );
    assert!(call(&application, "create_project", json!({"name":"File","path":directory.path().join("tandem.sqlite3"),"connection":{"kind":"local"}})).await.is_err());
    assert_eq!(
        call(&application, "snapshot", json!({})).await?["runtime"]["available"],
        false
    );
    Ok(())
}

#[tokio::test]
async fn failed_session_preparation_releases_running_state_for_recovery() -> Result<()> {
    let directory = TempDir::new()?;
    let app = app(&directory, Arc::new(MemoryVault::default()))?;
    let mut provider = call(&app, "save_provider", json!({"provider":provider()})).await?;
    let session = call(
        &app,
        "create_session",
        json!({
            "projectId":null,"providerId":provider["id"],"modelId":"model-one"
        }),
    )
    .await?;
    provider["models"][0]["id"] = json!("replacement-model");
    call(&app, "save_provider", json!({"provider":provider})).await?;
    assert!(
        call(
            &app,
            "send_message",
            json!({"sessionId":session["id"],"content":"cannot start","messageId":uuid::Uuid::new_v4()})
        )
        .await
        .is_err()
    );
    let snapshot = call(&app, "snapshot", json!({})).await?;
    assert_eq!(snapshot["sessions"][0]["status"], "error");
    assert_eq!(snapshot["runtime"]["available"], false);
    call(&app, "delete_session", json!({"sessionId":session["id"]})).await?;
    Ok(())
}

#[tokio::test]
async fn failed_credential_deletion_keeps_provider_available_for_retry() -> Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};

    struct LockedVault(AtomicBool);
    impl SecretStore for LockedVault {
        fn get(&self, _: &str) -> std::result::Result<Option<String>, SecretError> {
            Ok(None)
        }

        fn set(&self, _: &str, _: Option<&str>) -> std::result::Result<(), SecretError> {
            if self.0.load(Ordering::SeqCst) {
                return Err(SecretError::LockPoisoned);
            }
            Ok(())
        }

        fn get_telemetry_api_key(&self) -> std::result::Result<Option<String>, SecretError> {
            Ok(None)
        }

        fn set_telemetry_api_key(&self, _: Option<&str>) -> std::result::Result<(), SecretError> {
            Err(SecretError::LockPoisoned)
        }
    }

    let directory = TempDir::new()?;
    let secrets = Arc::new(LockedVault(AtomicBool::new(false)));
    let application = app(&directory, secrets.clone())?;
    let provider = call(
        &application,
        "save_provider",
        json!({"provider":provider()}),
    )
    .await?;
    secrets.0.store(true, Ordering::SeqCst);
    assert!(
        call(
            &application,
            "delete_provider",
            json!({"id":provider["id"]})
        )
        .await
        .is_err()
    );
    let snapshot = call(&application, "snapshot", json!({})).await?;
    assert_eq!(snapshot["settings"]["providers"][0]["id"], provider["id"]);
    Ok(())
}
