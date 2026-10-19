#![allow(clippy::expect_used)]

use github_copilot_sdk::{
    PermissionDecision, PermissionRequestData, PermissionRequestKind, handler::PermissionResult,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream, duplex},
    task::JoinHandle,
};

use super::*;
use crate::{ProviderProtocol, Runtime};

const TEST_TIMEOUT: Duration = Duration::from_secs(120);

struct Server {
    reader: BufReader<DuplexStream>,
    writer: DuplexStream,
    id: String,
}

impl Server {
    async fn request(&mut self) -> Value {
        timeout(TEST_TIMEOUT, async {
            let mut header = String::new();
            self.reader
                .read_line(&mut header)
                .await
                .expect("valid SDK fixture");
            let length: usize = header
                .trim()
                .strip_prefix("Content-Length: ")
                .expect("valid SDK fixture")
                .parse()
                .expect("valid SDK fixture");
            let mut separator = String::new();
            self.reader
                .read_line(&mut separator)
                .await
                .expect("valid SDK fixture");
            assert_eq!(separator, "\r\n");
            let mut body = vec![0; length];
            self.reader
                .read_exact(&mut body)
                .await
                .expect("valid SDK fixture");
            serde_json::from_slice(&body).expect("valid SDK fixture")
        })
        .await
        .expect("SDK request must arrive")
    }

    async fn write(&mut self, value: Value) {
        let body = serde_json::to_vec(&value).expect("valid SDK fixture");
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.writer
            .write_all(header.as_bytes())
            .await
            .expect("valid SDK fixture");
        self.writer
            .write_all(&body)
            .await
            .expect("valid SDK fixture");
        self.writer.flush().await.expect("valid SDK fixture");
    }

    async fn respond(&mut self, request: &Value, result: Value) {
        self.write(json!({"jsonrpc":"2.0", "id":request["id"], "result":result}))
            .await;
    }

    async fn reject(&mut self, request: &Value) {
        self.write(json!({
            "jsonrpc":"2.0", "id":request["id"],
            "error":{"code":-32000, "message":"private-provider-error"},
        }))
        .await;
    }

    async fn created(&mut self, request: &Value) {
        self.respond(request, json!({"sessionId":self.id})).await;
        let options = self.request().await;
        assert_eq!(options["method"], "session.options.update");
        assert_eq!(options["params"]["skipCustomInstructions"], true);
        self.respond(&options, json!({"success":true})).await;
    }

    async fn acknowledge(&mut self, method: &str) -> Value {
        let request = self.request().await;
        assert_eq!(request["method"], method);
        assert_eq!(request["params"]["sessionId"], self.id);
        self.respond(&request, json!({})).await;
        request
    }

    async fn event(&mut self, kind: &str, data: Value) {
        self.write(json!({
            "jsonrpc":"2.0", "method":"session.event",
            "params":{"sessionId":self.id,"event":{
                "id":kind,"timestamp":"2026-01-01T00:00:00Z", "type":kind,"data":data,
            }},
        }))
        .await;
    }

    async fn cleaned(&mut self, abort: bool) {
        if abort {
            self.acknowledge("session.abort").await;
        }
        self.acknowledge("session.detach").await;
        self.acknowledge("session.delete").await;
    }
}

fn settings(protocol: ProviderProtocol) -> (ProviderSettings, ModelSettings) {
    (
        ProviderSettings {
            protocol,
            base_url: "http://127.0.0.1:1/v1".into(),
            api_key: Some("fixture-only-not-a-credential".into()),
        },
        ModelSettings {
            id: "configured-model".into(),
            reasoning_effort: Some("low".into()),
            context_window: Some(1_000_000),
            max_output_tokens: (protocol == ProviderProtocol::Anthropic).then_some(128_000),
        },
    )
}

async fn start(protocol: ProviderProtocol) -> (Server, Value, JoinHandle<Result<String>>) {
    let (client_writer, reader) = duplex(65_536);
    let (writer, client_reader) = duplex(65_536);
    let directory = std::env::temp_dir();
    let client = Client::from_streams(client_reader, client_writer, directory.clone())
        .expect("valid SDK fixture");
    let runtime = Runtime {
        client,
        chat_directory: directory,
    };
    let (provider, model) = settings(protocol);
    let result = tokio::spawn(async move {
        runtime
            .summarize_title(provider, model, "TANDEM_TITLE_SUMMARY\n\"quoted input\"")
            .await
    });
    let mut server = Server {
        reader: BufReader::new(reader),
        writer,
        id: String::new(),
    };
    let request = server.request().await;
    assert_eq!(request["method"], "session.create");
    server.id = request["params"]["sessionId"]
        .as_str()
        .expect("valid SDK fixture")
        .to_owned();
    (server, request, result)
}

async fn outcome(task: JoinHandle<Result<String>>) -> Result<String> {
    timeout(TEST_TIMEOUT, task)
        .await
        .expect("valid SDK fixture")
        .expect("valid SDK fixture")
}

#[tokio::test(start_paused = true)]
async fn sends_no_tools_preserves_token_limits_and_returns_complete_text_after_deletion() {
    for protocol in [
        ProviderProtocol::Anthropic,
        ProviderProtocol::OpenaiResponses,
    ] {
        let (mut server, create, result) = start(protocol).await;
        let config = &create["params"];
        assert_eq!(config["availableTools"], json!([]));
        assert_eq!(config["tools"], json!([]));
        assert_eq!(config["systemMessage"]["mode"], "replace");
        assert_eq!(config["model"], "configured-model");
        assert_eq!(config["reasoningEffort"], "low");
        assert_eq!(
            config["modelCapabilities"]["limits"]["max_context_window_tokens"],
            1_000_000,
        );
        let expected_output = if protocol == ProviderProtocol::Anthropic {
            json!(128_000)
        } else {
            Value::Null
        };
        assert_eq!(config["provider"]["maxOutputTokens"], expected_output);
        assert_eq!(
            config["modelCapabilities"]["limits"]["max_output_tokens"],
            expected_output,
        );
        for field in [
            "enableConfigDiscovery",
            "enableFileHooks",
            "enableSkills",
            "enableHostGitOperations",
            "enableOnDemandInstructionDiscovery",
            "enableSessionTelemetry",
            "enableSessionStore",
            "requestExtensions",
            "requestCanvasRenderer",
        ] {
            assert_eq!(config[field], false, "{field}");
        }
        server.created(&create).await;
        let send = server.acknowledge("session.send").await;
        assert_eq!(
            send["params"]["prompt"],
            "TANDEM_TITLE_SUMMARY\n\"quoted input\""
        );
        server
            .event("assistant.message_delta", json!({"deltaContent":"partial"}))
            .await;
        server
            .event("assistant.message", json!({"content":"Complete title"}))
            .await;
        server.event("session.idle", json!({})).await;
        server.acknowledge("session.detach").await;
        let delete = server.request().await;
        assert_eq!(delete["method"], "session.delete");
        assert!(
            !result.is_finished(),
            "return only after deletion is confirmed"
        );
        server.respond(&delete, json!({})).await;
        assert_eq!(
            outcome(result).await.expect("valid SDK fixture"),
            "Complete title"
        );
    }
}

#[tokio::test]
async fn denies_every_permission_without_an_application_subscriber() {
    let (provider, model) = settings(ProviderProtocol::Anthropic);
    let config = configuration(&provider, &model, std::env::temp_dir(), temporary_id())
        .expect("valid SDK fixture");
    let handler = config.permission_handler.expect("valid SDK fixture");
    for kind in [
        PermissionRequestKind::Read,
        PermissionRequestKind::Write,
        PermissionRequestKind::Shell,
        PermissionRequestKind::Url,
        PermissionRequestKind::Mcp,
        PermissionRequestKind::CustomTool,
        PermissionRequestKind::Unknown,
    ] {
        let result = handler
            .handle(
                "summary".into(),
                "request".into(),
                PermissionRequestData {
                    kind: Some(kind),
                    ..Default::default()
                },
            )
            .await;
        assert!(
            matches!(
                result,
                PermissionResult::Decision {
                    decision: PermissionDecision::Reject(_),
                    ..
                }
            ),
            "{kind:?}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn cancellation_during_creation_or_inference_still_deletes_the_session() {
    for during_create in [true, false] {
        let (mut server, create, result) = start(ProviderProtocol::OpenaiResponses).await;
        if !during_create {
            server.created(&create).await;
            server.acknowledge("session.send").await;
        }
        result.abort();
        assert!(result.await.expect_err("fixture must fail").is_cancelled());
        if during_create {
            server.created(&create).await;
        }
        server.cleaned(true).await;
    }
}

#[tokio::test(start_paused = true)]
async fn inference_failures_and_timeout_abort_and_delete_without_exposing_provider_errors() {
    for failure in ["rpc", "event", "no-text", "timeout"] {
        let (mut server, create, result) = start(ProviderProtocol::OpenaiResponses).await;
        server.created(&create).await;
        let send = server.request().await;
        assert_eq!(send["method"], "session.send");
        if failure == "rpc" {
            server.reject(&send).await;
        } else {
            server.respond(&send, json!({})).await;
            match failure {
                "event" => {
                    server
                        .event("session.error", json!({"message":"private-provider-error"}))
                        .await;
                }
                "no-text" => server.event("session.idle", json!({})).await,
                "timeout" => (),
                _ => unreachable!(),
            }
        }
        server.cleaned(true).await;
        let error = outcome(result)
            .await
            .expect_err("fixture must fail")
            .to_string();
        assert!(!error.contains("private-provider-error"));
    }
}

#[tokio::test(start_paused = true)]
async fn creation_failure_still_attempts_deletion_and_cleanup_failure_is_reported() {
    for (creation_timeout, reject_delete) in [(false, false), (true, false), (false, true)] {
        let (mut server, create, result) = start(ProviderProtocol::OpenaiResponses).await;
        if !creation_timeout {
            server.reject(&create).await;
        }
        let delete = server.request().await;
        assert_eq!(delete["method"], "session.delete");
        assert_eq!(delete["params"]["sessionId"], server.id);
        if reject_delete {
            server.reject(&delete).await;
        } else {
            server.respond(&delete, json!({})).await;
        }
        let error = outcome(result)
            .await
            .expect_err("fixture must fail")
            .to_string();
        assert_eq!(
            error,
            if reject_delete {
                "title summary: temporary session could not be deleted"
            } else if creation_timeout {
                "title summary: session creation timed out"
            } else {
                "title summary: session creation failed"
            },
        );
    }
}
