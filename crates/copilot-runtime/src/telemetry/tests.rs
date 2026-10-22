use std::{
    collections::HashSet,
    io::{Read, Seek, Write},
    net::TcpListener,
    thread::JoinHandle,
};

use serde_json::{Value, json};

use super::*;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn settings(endpoint: String, capture_content: bool) -> TelemetrySettings {
    TelemetrySettings {
        endpoint,
        protocol: TelemetryProtocol::HttpJson,
        capture_content,
        agentlens: Some(AgentLensSettings {
            service_name: "tandem".into(),
            api_key: "fixture-agentlens-secret#not-a-credential".into(),
        }),
    }
}

fn record(operation: &str, id: &str, parent: Option<&str>, session: &str) -> Value {
    let mut value = json!({
        "type":"span","traceId":"11111111111111111111111111111111","spanId":id,
        "name":operation,"kind":0,"startTime":[1_700_000_000,100],"endTime":[1_700_000_001,200],
        "attributes":{"gen_ai.operation.name":operation,"gen_ai.conversation.id":session,
            "gen_ai.provider.name":"openai","gen_ai.request.model":"fixture-model",
            "gen_ai.usage.input_tokens":20,"gen_ai.usage.output_tokens":4},
        "status":{"code":0},"events":[],
    });
    if let Some(parent) = parent {
        value["parentSpanId"] = parent.into();
    }
    value
}

fn fields(span: &Value) -> serde_json::Map<String, Value> {
    span["attributes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| Some((item["key"].as_str()?.to_owned(), item["value"].clone())))
        .collect()
}

#[test]
fn maps_native_tree_with_full_messages_and_single_usage_accounting() -> TestResult {
    let root = record("invoke_agent", "2222222222222222", None, "real-session");
    let mut llm = record(
        "chat",
        "3333333333333333",
        Some("2222222222222222"),
        "real-session",
    );
    llm["attributes"]["gen_ai.request.stream"] = true.into();
    llm["attributes"]["gen_ai.response.time_to_first_chunk"] = 0.125.into();
    llm["attributes"]["gen_ai.response.finish_reasons"] = json!(["stop"]);
    llm["attributes"]["gen_ai.input.messages"] = json!([
        {"role":"user","parts":[{"type":"text","content":"Read the file"}]},
        {"role":"tool","parts":[{"type":"tool_call_response","response":{"text":"file contents"}}]},
    ])
    .to_string()
    .into();
    llm["attributes"]["gen_ai.system_instructions"] =
        json!([{"type":"text","content":"System instructions"}])
            .to_string()
            .into();
    llm["attributes"]["gen_ai.output.messages"] = json!([{"role":"assistant","parts":[
        {"type":"text","content":"Done"},{"type":"reasoning","content":"Reasoned"},
        {"type":"tool_call","id":"call-1","name":"view","arguments":{"path":"file.txt"}},
    ]}])
    .to_string()
    .into();
    llm["attributes"]["gen_ai.tool.definitions"] = json!([{"name":"view","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}]).to_string().into();
    let mut tool = record(
        "execute_tool",
        "4444444444444444",
        Some("2222222222222222"),
        "child-session",
    );
    tool["attributes"]["gen_ai.tool.name"] = "view".into();
    tool["attributes"]["gen_ai.tool.call.arguments"] = "{\"path\":\"file.txt\"}".into();
    tool["attributes"]["gen_ai.tool.call.result"] = "file contents".into();
    let output = transform::trace(&[llm, tool, root], "real-session")?;
    let llm = fields(&output[0]);
    assert_eq!(output[0]["kind"], 1);
    assert_eq!(output[2]["kind"], 2);
    assert_eq!(output[0]["startTimeUnixNano"], "1700000000000000100");
    assert_eq!(llm["gen_ai.span.kind"]["stringValue"], "LLM");
    assert_eq!(
        llm["gen_ai.response.time_to_first_token"]["intValue"],
        "125000000"
    );
    assert_eq!(llm["gen_ai.usage.total_tokens"]["intValue"], "24");
    assert_eq!(llm["gen_ai.prompts.0.role"]["stringValue"], "system");
    assert_eq!(
        llm["gen_ai.prompts.0.content"]["stringValue"],
        "System instructions"
    );
    assert_eq!(
        llm["gen_ai.prompts.2.content"]["stringValue"],
        "{\"text\":\"file contents\"}"
    );
    assert_eq!(
        llm["gen_ai.completion.0.reasoning_content"]["stringValue"],
        "Reasoned"
    );
    assert_eq!(
        llm["gen_ai.completion.0.tool_calls.0.arguments"]["stringValue"],
        "{\"path\":\"file.txt\"}"
    );
    assert_eq!(
        llm["gen_ai.completion.0.finish_reason"]["stringValue"],
        "stop"
    );
    let tool = fields(&output[1]);
    assert_eq!(tool["gen_ai.session.id"]["stringValue"], "real-session");
    assert_eq!(tool["tool.name"]["stringValue"], "view");
    assert!(
        tool["tool.parameters"]["stringValue"]
            .as_str()
            .is_some_and(|value| value.contains("properties"))
    );
    let root = fields(&output[2]);
    assert!(!root.contains_key("gen_ai.usage.input_tokens"));
    assert!(!root.contains_key("gen_ai.usage.output_tokens"));
    assert_eq!(
        root["gen_ai.user.time_to_first_token"]["intValue"],
        "125000000"
    );
    Ok(())
}

#[test]
fn queue_filters_titles_metrics_auxiliary_and_unregistered_sessions() -> TestResult {
    let registered = HashSet::from(["user-session".to_owned()]);
    let mut queue = Queue::new();
    for value in [
        json!({"type":"metric"}),
        json!({"type":"span","name":"session.timing"}),
        record(
            "invoke_agent",
            "2222222222222222",
            None,
            "tandem-title-helper",
        ),
        record("invoke_agent", "2222222222222222", None, "unknown-session"),
    ] {
        assert!(queue.push(value, 100, &registered)?.is_none());
    }
    let child = record(
        "chat",
        "3333333333333333",
        Some("2222222222222222"),
        "user-session",
    );
    assert!(queue.push(child, 100, &registered)?.is_none());
    let root = record("invoke_agent", "2222222222222222", None, "user-session");
    assert_eq!(
        queue.push(root, 100, &registered)?.map(|spans| spans.len()),
        Some(2)
    );
    assert!(queue.traces.is_empty());
    assert_eq!(queue.bytes, 0);
    Ok(())
}

#[test]
fn absent_native_subagent_input_uses_exact_task_instruction_without_inventing_llm_input()
-> TestResult {
    let mut task = record(
        "execute_tool",
        "4444444444444444",
        Some("2222222222222222"),
        "user-session",
    );
    task["attributes"]["gen_ai.tool.name"] = "task".into();
    task["attributes"]["gen_ai.tool.call.arguments"] =
        json!({"prompt":"Inspect the assigned file"})
            .to_string()
            .into();
    let agent = record(
        "invoke_agent",
        "5555555555555555",
        Some("4444444444444444"),
        "worker-session",
    );
    let llm = record(
        "chat",
        "6666666666666666",
        Some("5555555555555555"),
        "worker-session",
    );
    let output = transform::trace(&[task, agent, llm], "user-session")?;
    let agent = fields(&output[1]);
    assert_eq!(
        agent["input.value"]["stringValue"],
        "Inspect the assigned file"
    );
    assert_eq!(
        agent["tandem.input.source"]["stringValue"],
        "parent_task_prompt"
    );
    let llm = fields(&output[2]);
    assert!(!llm.contains_key("input.value"));
    assert!(!llm.contains_key("gen_ai.input.messages"));
    Ok(())
}

#[test]
fn malformed_boundaries_and_receiver_rejections_are_typed_and_redacted() {
    let config = settings("https://example.invalid:4318".into(), false);
    assert!(!format!("{config:?}").contains("fixture-agentlens-secret"));
    for (body, rejected) in [
        (r"{}", false),
        (r#"{"partialSuccess":{"rejectedSpans":"0"}}"#, false),
        (
            r#"{"partialSuccess":{"rejectedSpans":"2","errorMessage":"fixture-agentlens-secret"}}"#,
            true,
        ),
        (r#"{"partialSuccess":{"rejectedSpans":-1}}"#, true),
        ("invalid-json", true),
        ("[]", true),
    ] {
        let result = response_result(body.as_bytes());
        assert_eq!(result.is_err(), rejected, "{body}");
        if let Err(error) = result {
            assert!(!format!("{error:?} {error}").contains("fixture-agentlens-secret"));
        }
    }
    for field in ["traceId", "spanId", "startTime", "endTime"] {
        let mut invalid = record(
            "chat",
            "3333333333333333",
            Some("2222222222222222"),
            "user-session",
        );
        invalid[field] = Value::Null;
        assert!(matches!(
            transform::span(&invalid, "user-session"),
            Err(TelemetryError::InvalidSpan)
        ));
    }
    let mut queue = Queue::new();
    let child = record(
        "chat",
        "3333333333333333",
        Some("2222222222222222"),
        "user-session",
    );
    assert!(matches!(
        queue.push(child, MAX_PENDING_BYTES + 1, &HashSet::new()),
        Err(TelemetryError::QueueLimit)
    ));
}

fn receiver(
    statuses: Vec<u16>,
) -> std::io::Result<(String, JoinHandle<std::io::Result<Vec<Value>>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    let task = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for status in statuses {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte)?;
                header.push(byte[0]);
            }
            let header = String::from_utf8_lossy(&header);
            let length = header
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            let mut body = vec![0; length];
            stream.read_exact(&mut body)?;
            requests.push(serde_json::from_slice(&body)?);
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            )?;
        }
        Ok(requests)
    });
    Ok((endpoint, task))
}

#[tokio::test]
async fn sparse_spool_retries_complete_trace_and_cleans_up() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (endpoint, received) = receiver(vec![503, 200])?;
    let exporter = AgentLensExport::start(&settings(endpoint, true), directory.path()).await?;
    assert_eq!(exporter.configuration().capture_content, Some(true));
    exporter.register("user-session");
    let mut writer = std::fs::OpenOptions::new()
        .append(true)
        .open(&exporter.path)?;
    for value in [
        record(
            "chat",
            "3333333333333333",
            Some("2222222222222222"),
            "user-session",
        ),
        record("invoke_agent", "2222222222222222", None, "user-session"),
    ] {
        serde_json::to_writer(&mut writer, &value)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    drop(writer);
    exporter.finish().await?;
    let requests = received.join().map_err(|_| "receiver thread stopped")??;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    assert!(!exporter.path.exists());
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 0);
    Ok(())
}

#[tokio::test]
async fn incomplete_user_trace_is_reported_and_content_capture_can_be_disabled() -> TestResult {
    let directory = tempfile::tempdir()?;
    let exporter = AgentLensExport::start(
        &settings("http://127.0.0.1:1".into(), false),
        directory.path(),
    )
    .await?;
    assert_eq!(exporter.configuration().capture_content, Some(false));
    exporter.register("user-session");
    let mut writer = std::fs::OpenOptions::new()
        .append(true)
        .open(&exporter.path)?;
    let child = record(
        "chat",
        "3333333333333333",
        Some("2222222222222222"),
        "user-session",
    );
    serde_json::to_writer(&mut writer, &child)?;
    writer.write_all(b"\n")?;
    drop(writer);
    assert!(matches!(
        exporter.finish().await,
        Err(crate::Error::Telemetry(TelemetryError::IncompleteTrace {
            count: 1
        }))
    ));
    assert!(!exporter.path.exists());
    Ok(())
}

#[tokio::test]
async fn stop_after_inflight_eof_still_reports_late_incomplete_trace() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (temporary, file, path) = create_spool(directory.path(), "agentlens-")?;
    let (file, reached_eof, release_eof) =
        test_io::GatedEofFile::new(tokio::fs::File::from_std(file));
    let sender = Sender::new(&settings("http://127.0.0.1:1".into(), false))?;
    let sessions = Arc::new(RwLock::new(HashSet::from(["user-session".to_owned()])));
    let (stop, stopping) = watch::channel(false);
    let producer_path = path.clone();
    let producer = async move {
        reached_eof.await?;
        let mut writer = std::fs::OpenOptions::new()
            .append(true)
            .open(producer_path)?;
        let child = record(
            "chat",
            "3333333333333333",
            Some("2222222222222222"),
            "user-session",
        );
        serde_json::to_writer(&mut writer, &child)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        drop(writer);
        stop.send(true)?;
        release_eof.send(()).map_err(|()| "EOF reader stopped")?;
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            export_loop(path, file, sender, sessions, stopping),
            producer
        )
    })
    .await;
    temporary.close()?;
    let (exported, appended) = result?;
    appended?;
    assert!(matches!(
        exported,
        Err(TelemetryError::IncompleteTrace { count: 1 })
    ));
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 0);
    Ok(())
}

#[test]
fn sparse_reclaim_preserves_concurrent_append_offsets() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("spool");
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    #[cfg(windows)]
    prepare_sparse_file(&path)?;
    let mut writer = std::fs::OpenOptions::new().append(true).open(&path)?;
    writer.write_all(&vec![b'x'; 131_072])?;
    fsys::with(fsys::Method::Sync)?.punch_hole(&path, 0, 65_536)?;
    writer.write_all(b"appended-record")?;
    assert_eq!(file.metadata()?.len(), 131_087);
    let mut prefix = vec![1; 65_536];
    file.read_exact(&mut prefix)?;
    assert!(prefix.iter().all(|byte| *byte == 0));
    file.seek(std::io::SeekFrom::Start(131_072))?;
    let mut suffix = String::new();
    file.read_to_string(&mut suffix)?;
    assert_eq!(suffix, "appended-record");
    Ok(())
}

#[test]
fn transformed_tool_schema_amplification_stops_before_collecting_the_trace() -> TestResult {
    let mut definition = record(
        "chat",
        "3333333333333333",
        Some("2222222222222222"),
        "user-session",
    );
    definition["attributes"]["gen_ai.tool.definitions"] = json!([{
        "name":"view", "parameters":{"description":"x".repeat(256 * 1024)}
    }])
    .to_string()
    .into();
    let mut records = vec![definition];
    for _ in 0..128 {
        let mut tool = record(
            "execute_tool",
            "4444444444444444",
            Some("2222222222222222"),
            "user-session",
        );
        tool["attributes"]["gen_ai.tool.name"] = "view".into();
        records.push(tool);
    }
    // This later malformed record would return InvalidSpan if transformation
    // continued past the output budget (the entire native trace is under 1 MiB).
    records.push(json!({"attributes":{"gen_ai.operation.name":"invoke_agent"}}));
    assert!(serde_json::to_vec(&records)?.len() < 1024 * 1024);
    assert!(matches!(
        transform::trace(&records, "user-session"),
        Err(TelemetryError::QueueLimit)
    ));
    Ok(())
}
