use serde_json::{Map, Value, json};

use super::{EncodedSize, MAX_EXPORTED_TRACE_BYTES, TelemetryError};

pub(super) fn trace(records: &[Value], session: &str) -> Result<Vec<Value>, TelemetryError> {
    let definitions = records
        .iter()
        .filter_map(|record| record["attributes"]["gen_ai.tool.definitions"].as_str())
        .filter_map(|value| serde_json::from_str::<Vec<Value>>(value).ok())
        .flatten()
        .collect::<Vec<_>>();
    let mut spans = Vec::with_capacity(records.len());
    let mut size = EncodedSize(2); // JSON array brackets.
    for record in records {
        let mut record = record.clone();
        if record["attributes"]["gen_ai.operation.name"] == "execute_tool"
            && let Some(name) = record["attributes"]["gen_ai.tool.name"].as_str()
            && let Some(parameters) = definitions
                .iter()
                .find(|definition| definition["name"] == name)
                .and_then(|definition| definition.get("parameters"))
        {
            record["attributes"]["tool.parameters"] = parameters.to_string().into();
        }
        if record["attributes"]["gen_ai.operation.name"] == "invoke_agent"
            && record["attributes"].get("gen_ai.input.messages").is_none()
            && let Some(prompt) = parent_task_prompt(records, &record)
        {
            record["attributes"]["input.value"] = prompt.into();
            record["attributes"]["input.mime_type"] = "text/plain".into();
            record["attributes"]["tandem.input.source"] = "parent_task_prompt".into();
        }
        let transformed = span(&record, session)?;
        // Tool schemas and message aliases can amplify a small native trace.
        // Enforce the outbound budget as we build, not after collecting it.
        serde_json::to_writer(&mut size, &transformed).map_err(TelemetryError::InvalidRecord)?;
        size.0 = size.0.saturating_add(usize::from(!spans.is_empty()));
        if size.0 > MAX_EXPORTED_TRACE_BYTES {
            return Err(TelemetryError::QueueLimit);
        }
        spans.push(transformed);
    }
    let first_token = records
        .iter()
        .filter(|record| record["attributes"]["gen_ai.operation.name"] == "chat")
        .filter_map(|record| {
            let start = timestamp(&record["startTime"]).ok()?;
            let seconds = record["attributes"]["gen_ai.response.time_to_first_chunk"].as_f64()?;
            start.checked_add(nanoseconds(seconds)?)
        })
        .min();
    if let Some(root) = spans
        .iter_mut()
        .find(|span| span.get("parentSpanId").is_none())
        && let Some(duration) = first_token
            .and_then(|first| first.checked_sub(root["startTimeUnixNano"].as_str()?.parse().ok()?))
        && let Some(attributes) = root["attributes"].as_array_mut()
    {
        attributes.push(json!({"key":"gen_ai.user.time_to_first_token","value":{"intValue":duration.to_string()}}));
    }
    Ok(spans)
}

fn parent_task_prompt(records: &[Value], agent: &Value) -> Option<String> {
    let parent = agent["parentSpanId"].as_str()?;
    let task = records.iter().find(|record| {
        record["spanId"] == parent
            && record["attributes"]["gen_ai.operation.name"] == "execute_tool"
            && record["attributes"]["gen_ai.tool.name"] == "task"
    })?;
    let arguments: Value =
        serde_json::from_str(task["attributes"]["gen_ai.tool.call.arguments"].as_str()?).ok()?;
    arguments["prompt"].as_str().map(str::to_owned)
}

pub(super) fn span(record: &Value, session: &str) -> Result<Value, TelemetryError> {
    let mut attributes = record["attributes"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let operation = attributes
        .get("gen_ai.operation.name")
        .and_then(Value::as_str)
        .ok_or(TelemetryError::InvalidSpan)?
        .to_owned();
    let parent = record["parentSpanId"]
        .as_str()
        .filter(|id| !id.is_empty() && *id != "0000000000000000");
    let category = match operation.as_str() {
        "invoke_agent" | "invoke_workflow" => "AGENT",
        "chat" => "LLM",
        "execute_tool" => "TOOL",
        _ => return Err(TelemetryError::InvalidSpan),
    };
    attributes.insert("gen_ai.span.kind".into(), category.into());
    attributes.insert("gen_ai.session.id".into(), session.into());
    copy(&mut attributes, "gen_ai.provider.name", "gen_ai.system");
    copy(&mut attributes, "gen_ai.request.model", "gen_ai.model_name");
    match operation.as_str() {
        "chat" => llm(&mut attributes),
        "execute_tool" => {
            copy(&mut attributes, "gen_ai.tool.name", "tool.name");
            copy(
                &mut attributes,
                "gen_ai.tool.description",
                "tool.description",
            );
            copy(&mut attributes, "gen_ai.tool.call.arguments", "input.value");
            copy(&mut attributes, "gen_ai.tool.call.result", "output.value");
        }
        _ => {
            // Native invoke_agent spans aggregate their chat children. AgentLens
            // counts usage on LLM spans, so retaining aggregates double-counts.
            attributes.retain(|key, _| !key.starts_with("gen_ai.usage."));
            content(&mut attributes);
        }
    }
    let mut span = json!({
        "traceId":identifier(&record["traceId"],32)?,
        "spanId":identifier(&record["spanId"],16)?,
        "name":record["name"].as_str().ok_or(TelemetryError::InvalidSpan)?,
        "kind":if parent.is_none() {2} else {1},
        "startTimeUnixNano":timestamp(&record["startTime"])? .to_string(),
        "endTimeUnixNano":timestamp(&record["endTime"])? .to_string(),
        "attributes":attributes.iter().map(|(key,value)|json!({"key":key,"value":any(value)})).collect::<Vec<_>>(),
        "status":{"code":record["status"]["code"].as_u64().unwrap_or(0)},
    });
    if let Some(parent) = parent {
        span["parentSpanId"] = identifier(&Value::String(parent.into()), 16)?.into();
    }
    if let Some(events) = record["events"].as_array() {
        span["events"] = events.iter().filter_map(|event| {
            let attributes=event["attributes"].as_object()?;
            Some(json!({
                "name":event["name"].as_str()?,
                "timeUnixNano":timestamp(&event["time"]).ok()?.to_string(),
                "attributes":attributes.iter().map(|(key,value)|json!({"key":key,"value":any(value)})).collect::<Vec<_>>(),
            }))
        }).collect();
    }
    Ok(span)
}

fn identifier(value: &Value, length: usize) -> Result<&str, TelemetryError> {
    value
        .as_str()
        .filter(|id| {
            id.len() == length
                && id.bytes().all(|byte| byte.is_ascii_hexdigit())
                && id.bytes().any(|byte| byte != b'0')
        })
        .ok_or(TelemetryError::InvalidSpan)
}

fn timestamp(value: &Value) -> Result<u64, TelemetryError> {
    let parts = value
        .as_array()
        .filter(|parts| parts.len() == 2)
        .ok_or(TelemetryError::InvalidSpan)?;
    let seconds = parts[0].as_u64().ok_or(TelemetryError::InvalidSpan)?;
    let nanos = parts[1]
        .as_u64()
        .filter(|nanos| *nanos < 1_000_000_000)
        .ok_or(TelemetryError::InvalidSpan)?;
    seconds
        .checked_mul(1_000_000_000)
        .and_then(|seconds| seconds.checked_add(nanos))
        .ok_or(TelemetryError::InvalidSpan)
}

fn copy(attributes: &mut Map<String, Value>, source: &str, target: &str) {
    if let Some(value) = attributes.get(source).cloned() {
        attributes.insert(target.into(), value);
    }
}

fn content(attributes: &mut Map<String, Value>) {
    for (source, target) in [
        ("gen_ai.input.messages", "input.value"),
        ("gen_ai.output.messages", "output.value"),
    ] {
        copy(attributes, source, target);
        if attributes.contains_key(target) {
            attributes.insert(
                format!("{}.mime_type", target.trim_end_matches(".value")),
                "application/json".into(),
            );
        }
    }
}

fn llm(attributes: &mut Map<String, Value>) {
    content(attributes);
    copy(
        attributes,
        "gen_ai.request.stream",
        "gen_ai.request.is_stream",
    );
    let mut parameters = Map::new();
    for (source, target) in [
        ("gen_ai.request.temperature", "temperature"),
        ("gen_ai.request.top_p", "top_p"),
        ("gen_ai.request.max_tokens", "max_tokens"),
        ("gen_ai.request.stop_sequences", "stop"),
    ] {
        if let Some(value) = attributes.get(source) {
            parameters.insert(target.into(), value.clone());
        }
    }
    if !parameters.is_empty() {
        attributes.insert(
            "gen_ai.request.parameters".into(),
            Value::Object(parameters).to_string().into(),
        );
    }
    if let (Some(input), Some(output)) = (
        attributes
            .get("gen_ai.usage.input_tokens")
            .and_then(Value::as_u64),
        attributes
            .get("gen_ai.usage.output_tokens")
            .and_then(Value::as_u64),
    ) && let Some(total) = input.checked_add(output)
    {
        attributes.insert("gen_ai.usage.total_tokens".into(), total.into());
    }
    if let Some(nanos) = attributes
        .get("gen_ai.response.time_to_first_chunk")
        .and_then(Value::as_f64)
        .and_then(nanoseconds)
    {
        attributes.insert("gen_ai.response.time_to_first_token".into(), nanos.into());
    }
    messages(attributes);
}

fn messages(attributes: &mut Map<String, Value>) {
    for (source, prefix) in [
        ("gen_ai.input.messages", "gen_ai.prompts"),
        ("gen_ai.output.messages", "gen_ai.completion"),
    ] {
        let Some(mut messages) = attributes
            .get(source)
            .and_then(Value::as_str)
            .and_then(|encoded| serde_json::from_str::<Vec<Value>>(encoded).ok())
        else {
            continue;
        };
        if source == "gen_ai.input.messages"
            && let Some(parts) = attributes
                .get("gen_ai.system_instructions")
                .and_then(Value::as_str)
                .and_then(|encoded| serde_json::from_str::<Vec<Value>>(encoded).ok())
        {
            messages.insert(0, json!({"role":"system","parts":parts}));
        }
        for (index, message) in messages.iter().enumerate() {
            let prefix = format!("{prefix}.{index}");
            if source == "gen_ai.output.messages"
                && let Some(reason) = attributes
                    .get("gen_ai.response.finish_reasons")
                    .and_then(Value::as_array)
                    .and_then(|reasons| reasons.get(index))
                    .cloned()
            {
                attributes.insert(format!("{prefix}.finish_reason"), reason);
            }
            message_fields(attributes, &prefix, message);
        }
    }
}

fn message_fields(attributes: &mut Map<String, Value>, prefix: &str, message: &Value) {
    if let Some(role) = message["role"].as_str() {
        attributes.insert(format!("{prefix}.role"), role.into());
    }
    let Some(parts) = message["parts"].as_array() else {
        return;
    };
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_index = 0;
    for part in parts {
        match part["type"].as_str() {
            Some("text") => {
                if let Some(content) = part["content"].as_str() {
                    text.push_str(content);
                }
            }
            Some("reasoning") => {
                if let Some(content) = part["content"].as_str() {
                    reasoning.push_str(content);
                }
            }
            Some("tool_call_response") => {
                if let Some(response) = part.get("response") {
                    text.push_str(
                        &response
                            .as_str()
                            .map_or_else(|| response.to_string(), str::to_owned),
                    );
                }
            }
            Some("tool_call") => {
                for (source, target) in [("id", "id"), ("name", "name"), ("arguments", "arguments")]
                {
                    if let Some(value) = part.get(source) {
                        let value = value
                            .as_str()
                            .map_or_else(|| value.to_string(), str::to_owned);
                        attributes.insert(
                            format!("{prefix}.tool_calls.{tool_index}.{target}"),
                            value.into(),
                        );
                    }
                }
                tool_index += 1;
            }
            _ => {
                text.push_str(&part.to_string());
            }
        }
    }
    attributes.insert(format!("{prefix}.content"), text.into());
    if !reasoning.is_empty() {
        attributes.insert(format!("{prefix}.reasoning_content"), reasoning.into());
    }
}

fn nanoseconds(seconds: f64) -> Option<u64> {
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    // Parse the rounded integer representation to avoid unchecked float casts.
    format!("{:.0}", seconds * 1_000_000_000.0)
        .parse::<i64>()
        .ok()
        .and_then(|value| value.try_into().ok())
}

fn any(value: &Value) -> Value {
    match value {
        Value::String(text) => json!({"stringValue":text}),
        Value::Bool(flag) => json!({"boolValue":flag}),
        Value::Number(number) if number.is_i64() || number.is_u64() => {
            json!({"intValue":number.to_string()})
        }
        Value::Number(number) => json!({"doubleValue":number}),
        Value::Array(values) => {
            json!({"arrayValue":{"values":values.iter().map(any).collect::<Vec<_>>()}})
        }
        _ => json!({"stringValue":value.to_string()}),
    }
}
