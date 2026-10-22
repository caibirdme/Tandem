use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use github_copilot_sdk::Client;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream, duplex};

use crate::{
    AttachmentError, Error, MessageAttachment, ModelSettings, PermissionMode, ProviderProtocol,
    ProviderSettings, Runtime, SessionMode, SessionOptions, SkillError, SkillSelection,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct SdkServer {
    reader: BufReader<DuplexStream>,
    writer: DuplexStream,
}

impl SdkServer {
    async fn request(&mut self, method: &str) -> TestResult<Value> {
        let request: Value = tokio::time::timeout(Duration::from_secs(10), async {
            let mut header = String::new();
            self.reader.read_line(&mut header).await?;
            let length: usize = header
                .trim()
                .strip_prefix("Content-Length: ")
                .ok_or("missing SDK frame length")?
                .parse()?;
            let mut separator = String::new();
            self.reader.read_line(&mut separator).await?;
            assert_eq!(separator, "\r\n");
            let mut body = vec![0; length];
            self.reader.read_exact(&mut body).await?;
            Ok::<Value, Box<dyn std::error::Error + Send + Sync>>(serde_json::from_slice(&body)?)
        })
        .await??;
        assert_eq!(request["method"], method);
        Ok(request)
    }

    async fn respond(&mut self, request: &Value, result: Value) -> TestResult {
        self.write_response(json!({"jsonrpc":"2.0", "id":request["id"], "result":result}))
            .await
    }

    async fn respond_error(&mut self, request: &Value, error: Value) -> TestResult {
        self.write_response(json!({"jsonrpc":"2.0", "id":request["id"], "error":error}))
            .await
    }

    async fn write_response(&mut self, response: Value) -> TestResult {
        let body = serde_json::to_vec(&response)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.writer.write_all(header.as_bytes()).await?;
        self.writer.write_all(&body).await?;
        self.writer.flush().await?;
        Ok(())
    }

    async fn reply(&mut self, method: &str, response: Value) -> TestResult<Value> {
        let request = self.request(method).await?;
        self.respond(&request, response).await?;
        Ok(request)
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> TestResult<Self> {
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "tandem-message-test-{}-{timestamp}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&directory)?;
        Ok(Self(directory))
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn session_options(directory: PathBuf, work: bool) -> SessionOptions {
    SessionOptions {
        session_id: Some("attachment-session".into()),
        mode: if work {
            SessionMode::Work { directory }
        } else {
            SessionMode::Chat
        },
        permission_mode: PermissionMode::Ask,
        provider: ProviderSettings {
            protocol: ProviderProtocol::OpenaiResponses,
            base_url: "http://127.0.0.1:1/v1".into(),
            api_key: None,
        },
        model: ModelSettings {
            id: "fixture-model".into(),
            context_window: 200_000,
            ..Default::default()
        },
        system_prompt: None,
    }
}

async fn serve_skill(server: &mut SdkServer, skill_path: &str) -> TestResult {
    server
        .reply("session.skills.reload", json!({"warnings":[],"errors":[]}))
        .await?;
    server
        .reply("session.skills.ensureLoaded", json!({}))
        .await?;
    server
        .reply(
            "session.skills.list",
            json!({"skills":[{
                "name":"review","description":"Review","enabled":true,
                "path":skill_path,"source":"project","userInvocable":true,
            }]}),
        )
        .await?;
    server
        .reply(
            "session.commands.list",
            json!({"commands":[{
                "name":"review","description":"Review","kind":"skill",
                "allowDuringAgentExecution":false,
            }]}),
        )
        .await?;
    let invocation = server
        .reply(
            "session.commands.invoke",
            json!({
                "kind":"agent-prompt","displayPrompt":"Review",
                "prompt":"Expanded skill instructions",
            }),
        )
        .await?;
    assert_eq!(invocation["params"]["name"], "review");
    Ok(())
}

#[tokio::test]
async fn attachment_only_and_skill_messages_reach_sdk_with_original_blobs() -> TestResult {
    for work in [false, true] {
        let directory = TestDirectory::new()?;
        let canonical = std::fs::canonicalize(&directory.0)?;
        let skill = canonical.join(".agents/skills/review/SKILL.md");
        std::fs::create_dir_all(skill.parent().ok_or("skill directory")?)?;
        std::fs::write(&skill, "Review the attached file.")?;
        let skill_path = skill.to_string_lossy().into_owned();
        let selected = if work {
            vec![SkillSelection {
                name: "review".into(),
                path: skill_path.clone(),
            }]
        } else {
            Vec::new()
        };
        let attachments = vec![
            MessageAttachment::from_bytes("图像.png", "image/png", &[0, 1, 128, 255]),
            MessageAttachment::from_bytes("notes.txt", "text/plain", b"attached text"),
        ];
        let expected_attachments: Vec<Value> = attachments
            .iter()
            .map(|attachment| {
                json!({
                    "type":"blob",
                    "data":attachment.data,
                    "mimeType":attachment.mime_type,
                    "displayName":attachment.name,
                })
            })
            .collect();
        let (client_writer, reader) = duplex(65_536);
        let (writer, client_reader) = duplex(65_536);
        let client = Client::from_streams(client_reader, client_writer, canonical.clone())?;
        let runtime = Runtime {
            agentlens: None,
            client,
            chat_directory: canonical.clone(),
        };
        let mut server = SdkServer {
            reader: BufReader::new(reader),
            writer,
        };
        let submit = tokio::spawn(async move {
            let session = runtime
                .create_session(session_options(canonical, work))
                .await?;
            let prompt = if work { "Review this" } else { "" };
            let prepared = session
                .prepare_message(prompt, &selected, &attachments, false)
                .await?;
            assert!(!session.lifecycle.is_running());
            let id = session.send_prepared(prepared).await?;
            session.disconnect().await?;
            Ok::<_, Error>(id)
        });
        server
            .reply("session.create", json!({"sessionId":"attachment-session"}))
            .await?;
        if work {
            serve_skill(&mut server, &skill_path).await?;
        }
        let send = server
            .reply("session.send", json!({"messageId":"attachment-message"}))
            .await?;
        assert_eq!(send["params"]["attachments"], json!(expected_attachments));
        assert_eq!(
            send["params"]["prompt"],
            if work {
                "Expanded skill instructions\n\nReview this"
            } else {
                ""
            }
        );
        if work {
            assert_eq!(send["params"]["displayPrompt"], "Review this");
        }
        server
            .reply("session.detach", json!({"success":true}))
            .await?;
        assert_eq!(submit.await??, "attachment-message");
    }
    Ok(())
}

#[tokio::test]
async fn unsupported_pdf_is_rejected_before_the_sdk_receives_a_message() -> TestResult {
    let directory = TestDirectory::new()?;
    let (client_writer, reader) = duplex(65_536);
    let (writer, client_reader) = duplex(65_536);
    let client = Client::from_streams(client_reader, client_writer, directory.0.clone())?;
    let runtime = Runtime {
        agentlens: None,
        client,
        chat_directory: directory.0.clone(),
    };
    let mut options = session_options(directory.0.clone(), false);
    options.provider.protocol = ProviderProtocol::OpenaiCompletions;
    let submit = tokio::spawn(async move {
        let session = runtime.create_session(options).await?;
        let attachment =
            MessageAttachment::from_bytes("report.pdf", "application/octet-stream", b"%PDF-1.4");
        assert!(matches!(
            session.prepare_message("", &[], &[attachment], false).await,
            Err(Error::Attachments(
                AttachmentError::PdfUnsupportedByProvider
            ))
        ));
        assert!(!session.lifecycle.is_running());
        session.disconnect().await
    });
    let mut server = SdkServer {
        reader: BufReader::new(reader),
        writer,
    };
    server
        .reply("session.create", json!({"sessionId":"attachment-session"}))
        .await?;
    // No session.send may occur between creation and disconnect.
    server
        .reply("session.detach", json!({"success":true}))
        .await?;
    submit.await??;
    Ok(())
}

async fn serve_empty_skills(server: &mut SdkServer) -> TestResult {
    server
        .reply("session.skills.reload", json!({"warnings":[],"errors":[]}))
        .await?;
    server
        .reply("session.skills.ensureLoaded", json!({}))
        .await?;
    server
        .reply("session.skills.list", json!({"skills":[]}))
        .await?;
    Ok(())
}

#[tokio::test]
async fn fleet_is_explicit_and_restores_the_tool_boundary_for_the_next_turn() -> TestResult {
    let directory = TestDirectory::new()?;
    let canonical = std::fs::canonicalize(&directory.0)?;
    let (client_writer, reader) = duplex(65_536);
    let (writer, client_reader) = duplex(65_536);
    let runtime = Runtime {
        agentlens: None,
        client: Client::from_streams(client_reader, client_writer, canonical.clone())?,
        chat_directory: canonical.clone(),
    };
    let attachment = MessageAttachment::from_bytes("notes.txt", "text/plain", b"Fleet attachment");
    let expected_attachment = json!({"type":"blob", "data":attachment.data, "mimeType":"text/plain", "displayName":"notes.txt"});
    let submit = tokio::spawn(async move {
        let session = runtime
            .create_session(session_options(canonical, true))
            .await?;
        for fleet in [false, true, false] {
            let prepared = session
                .prepare_message(
                    "Inspect independent packages",
                    &[],
                    std::slice::from_ref(&attachment),
                    fleet,
                )
                .await?;
            assert!(!session.lifecycle.is_running());
            session.send_prepared(prepared).await?;
            // The real runtime closes this lifecycle through session.idle.
            session.lifecycle.finish_turn();
        }
        session.disconnect().await
    });
    let mut server = SdkServer {
        reader: BufReader::new(reader),
        writer,
    };
    let create = server
        .reply("session.create", json!({"sessionId":"attachment-session"}))
        .await?;
    let initial = create["params"]["availableTools"]
        .as_array()
        .ok_or("missing tools")?;
    assert!(!initial.contains(&json!("builtin:task")));
    for fleet in [false, true, false].into_iter().enumerate() {
        let (index, enabled) = fleet;
        serve_empty_skills(&mut server).await?;
        if index > 0 {
            let update = server
                .reply("session.options.update", json!({"success":true}))
                .await?;
            let tools = update["params"]["availableTools"]
                .as_array()
                .ok_or("missing updated tools")?;
            for name in ["task", "read_agent", "write_agent", "list_agents", "sql"] {
                assert_eq!(tools.contains(&json!(format!("builtin:{name}"))), enabled);
            }
        }
        let request = if enabled {
            server
                .reply("session.fleet.start", json!({"started":true}))
                .await?
        } else {
            server
                .reply("session.send", json!({"messageId":"ordinary-message"}))
                .await?
        };
        assert_eq!(request["params"]["prompt"], "Inspect independent packages");
        assert_eq!(
            request["params"]["attachments"],
            json!([expected_attachment])
        );
        if enabled {
            assert_eq!(request["params"]["wait"], false);
        }
    }
    server
        .reply("session.detach", json!({"success":true}))
        .await?;
    submit.await??;
    Ok(())
}

#[tokio::test]
async fn fleet_cannot_expand_chat_capabilities() -> TestResult {
    let directory = TestDirectory::new()?;
    let (client_writer, reader) = duplex(65_536);
    let (writer, client_reader) = duplex(65_536);
    let runtime = Runtime {
        agentlens: None,
        client: Client::from_streams(client_reader, client_writer, directory.0.clone())?,
        chat_directory: directory.0.clone(),
    };
    let submit = tokio::spawn(async move {
        let session = runtime
            .create_session(session_options(directory.0.clone(), false))
            .await?;
        assert!(matches!(
            session
                .prepare_message("Parallel research", &[], &[], true)
                .await,
            Err(Error::InvalidConfiguration(
                crate::ConfigurationError::FleetUnavailableInChat
            ))
        ));
        assert!(!session.lifecycle.is_running());
        session.disconnect().await
    });
    let mut server = SdkServer {
        reader: BufReader::new(reader),
        writer,
    };
    server
        .reply("session.create", json!({"sessionId":"attachment-session"}))
        .await?;
    server
        .reply("session.detach", json!({"success":true}))
        .await?;
    submit.await??;
    Ok(())
}

#[tokio::test]
async fn rejected_fleet_setup_never_starts_inference_and_releases_the_reservation() -> TestResult {
    for tool_update_succeeds in [false, true] {
        let directory = TestDirectory::new()?;
        let canonical = std::fs::canonicalize(&directory.0)?;
        let (client_writer, reader) = duplex(65_536);
        let (writer, client_reader) = duplex(65_536);
        let runtime = Runtime {
            agentlens: None,
            client: Client::from_streams(client_reader, client_writer, canonical.clone())?,
            chat_directory: canonical.clone(),
        };
        let submit = tokio::spawn(async move {
            let session = runtime
                .create_session(session_options(canonical, true))
                .await?;
            let prepared = session
                .prepare_message("Parallel research", &[], &[], true)
                .await?;
            let error = session.send_prepared(prepared).await;
            assert!(matches!(
                (tool_update_succeeds, error),
                (false, Err(Error::FleetToolsRejected)) | (true, Err(Error::FleetNotStarted))
            ));
            assert!(!session.lifecycle.is_running());
            assert!(session.submission.try_lock().is_ok());
            session.disconnect().await
        });
        let mut server = SdkServer {
            reader: BufReader::new(reader),
            writer,
        };
        server
            .reply("session.create", json!({"sessionId":"attachment-session"}))
            .await?;
        serve_empty_skills(&mut server).await?;
        server
            .reply(
                "session.options.update",
                json!({"success":tool_update_succeeds}),
            )
            .await?;
        if tool_update_succeeds {
            server
                .reply("session.fleet.start", json!({"started":false}))
                .await?;
        }
        server
            .reply("session.detach", json!({"success":true}))
            .await?;
        submit.await??;
    }
    Ok(())
}

#[tokio::test]
async fn skill_discovery_preserves_sdk_failures_and_can_retry_an_empty_catalog() -> TestResult {
    let directory = TestDirectory::new()?;
    let canonical = std::fs::canonicalize(&directory.0)?;
    let root = canonical.join(".agents/skills");
    std::fs::create_dir_all(&root)?;
    let (client_writer, reader) = duplex(65_536);
    let (writer, client_reader) = duplex(65_536);
    let client = Client::from_streams(client_reader, client_writer, canonical.clone())?;
    let runtime = Runtime {
        agentlens: None,
        client,
        chat_directory: canonical.clone(),
    };
    let mut server = SdkServer {
        reader: BufReader::new(reader),
        writer,
    };
    let discovery = tokio::spawn(async move {
        let failed = runtime.list_project_skills(&canonical).await;
        let retried = runtime.list_project_skills(&canonical).await;
        (failed, retried)
    });
    let request = server.request("skills.discover").await?;
    assert_eq!(request["params"]["excludeHostSkills"], true);
    assert_eq!(request["params"]["projectPaths"], json!([]));
    assert!(
        request["params"]["skillDirectories"]
            .as_array()
            .ok_or("missing skill directories")?
            .contains(&json!(root.to_string_lossy()))
    );
    server
        .respond_error(
            &request,
            json!({"code":-32603,"message":"Skill discovery failed"}),
        )
        .await?;
    server
        .reply("skills.discover", json!({"skills":[]}))
        .await?;
    let (failed, retried) = discovery.await?;
    assert!(matches!(failed, Err(Error::Sdk(_))));
    assert_eq!(retried?, Vec::<crate::ProjectSkill>::new());
    Ok(())
}

#[tokio::test]
async fn skill_discovery_timeout_remains_distinct_from_an_empty_catalog() -> TestResult {
    let directory = TestDirectory::new()?;
    let canonical = std::fs::canonicalize(&directory.0)?;
    std::fs::create_dir_all(canonical.join(".agents/skills"))?;
    let (client_writer, reader) = duplex(65_536);
    let (writer, client_reader) = duplex(65_536);
    let client = Client::from_streams(client_reader, client_writer, canonical.clone())?;
    let runtime = Runtime {
        agentlens: None,
        client,
        chat_directory: canonical.clone(),
    };
    let mut server = SdkServer {
        reader: BufReader::new(reader),
        writer,
    };
    let discovery = tokio::spawn(async move { runtime.list_project_skills(&canonical).await });
    server.request("skills.discover").await?;
    assert!(!discovery.is_finished());
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(15)).await;
    let result = discovery.await?;
    tokio::time::resume();
    assert!(matches!(
        result,
        Err(Error::Skills(SkillError::DiscoveryTimeout))
    ));
    Ok(())
}

#[tokio::test]
async fn force_stop_releases_sessions_after_graceful_detach_is_cancelled() -> TestResult {
    let directory = TestDirectory::new()?;
    let (client_writer, reader) = duplex(65_536);
    let (writer, client_reader) = duplex(65_536);
    let runtime = Runtime {
        agentlens: None,
        client: Client::from_streams(client_reader, client_writer, directory.0.clone())?,
        chat_directory: directory.0.clone(),
    };
    let mut server = SdkServer {
        reader: BufReader::new(reader),
        writer,
    };
    let (session, created) = tokio::join!(
        runtime.create_session(session_options(directory.0.clone(), false)),
        server.reply("session.create", json!({"sessionId":"attachment-session"})),
    );
    created?;
    let session = session?;
    let mut graceful = Box::pin(runtime.stop());
    tokio::select! {
        result = &mut graceful => panic!("shutdown completed without a detach response: {result:?}"),
        request = server.request("session.detach") => { request?; }
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(1), &mut graceful)
            .await
            .is_err()
    );
    drop(graceful);
    runtime.force_stop();
    assert!(!runtime.is_healthy().await);
    tokio::time::timeout(Duration::from_secs(1), async {
        while session.is_connected() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let mut remaining = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(1),
        server.reader.read_to_end(&mut remaining),
    )
    .await??;
    Ok(())
}
