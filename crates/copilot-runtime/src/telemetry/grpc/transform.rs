use opentelemetry_proto::tonic::{metrics::v1::ResourceMetrics, trace::v1::ResourceSpans};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::GrpcError;

fn invalid(field: &'static str) -> GrpcError {
    GrpcError::InvalidRecord { field }
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T, GrpcError> {
    // Deserialization diagnostics may echo content. Retain only the failed stage.
    serde_json::from_value(value).map_err(|_| invalid("OTLP encoding"))
}

fn timestamp(value: &Value) -> Result<String, GrpcError> {
    let parts = value
        .as_array()
        .filter(|parts| parts.len() == 2)
        .ok_or_else(|| invalid("timestamp"))?;
    let seconds = parts[0].as_u64().ok_or_else(|| invalid("timestamp"))?;
    let nanos = parts[1]
        .as_u64()
        .filter(|nanos| *nanos < 1_000_000_000)
        .ok_or_else(|| invalid("timestamp"))?;
    seconds
        .checked_mul(1_000_000_000)
        .and_then(|seconds| seconds.checked_add(nanos))
        .map(|nanos| nanos.to_string())
        .ok_or_else(|| invalid("timestamp"))
}

fn identifier(value: &Value, length: usize, allow_empty: bool) -> Result<(), GrpcError> {
    let id = value.as_str().ok_or_else(|| invalid("span identifier"))?;
    if allow_empty && (id.is_empty() || id.len() == length && id.bytes().all(|byte| byte == b'0')) {
        return Ok(());
    }
    if id.len() != length
        || !id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || id.bytes().all(|byte| byte == b'0')
    {
        return Err(invalid("span identifier"));
    }
    Ok(())
}

fn any(value: &Value) -> Result<Value, GrpcError> {
    Ok(match value {
        Value::String(value) => json!({"stringValue":value}),
        Value::Bool(value) => json!({"boolValue":value}),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                json!({"intValue":value.to_string()})
            } else if value.is_u64() {
                return Err(invalid("integer attribute"));
            } else {
                json!({"doubleValue":value})
            }
        }
        Value::Array(values) => {
            let values = values.iter().map(any).collect::<Result<Vec<_>, _>>()?;
            json!({"arrayValue":{"values":values}})
        }
        Value::Object(_) => json!({"kvlistValue":{"values":attributes(value)?}}),
        Value::Null => json!({}),
    })
}

fn attributes(value: &Value) -> Result<Value, GrpcError> {
    if value.is_null() {
        return Ok(json!([]));
    }
    value
        .as_object()
        .ok_or_else(|| invalid("attributes"))?
        .iter()
        .map(|(key, value)| Ok(json!({"key":key,"value":any(value)?})))
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn resource(record: &Value) -> Result<Value, GrpcError> {
    let mut resource = record["resource"].clone();
    if resource.is_null() {
        resource = json!({});
    }
    if !resource.is_object() {
        return Err(invalid("resource"));
    }
    resource["attributes"] = attributes(&resource["attributes"])?;
    Ok(resource)
}

fn scope(record: &Value) -> Result<Value, GrpcError> {
    let mut scope = record["instrumentationScope"].clone();
    if scope.is_null() {
        scope = json!({});
    }
    if !scope.is_object() {
        return Err(invalid("instrumentation scope"));
    }
    scope["attributes"] = attributes(&scope["attributes"])?;
    Ok(scope)
}

pub(super) fn span(mut record: Value) -> Result<ResourceSpans, GrpcError> {
    if !record.is_object() {
        return Err(invalid("span"));
    }
    let resource = resource(&record)?;
    let scope = scope(&record)?;
    identifier(&record["traceId"], 32, false)?;
    identifier(&record["spanId"], 16, false)?;
    if let Some(parent) = record.get("parentSpanId") {
        identifier(parent, 16, true)?;
        if parent.as_str().is_some_and(str::is_empty) {
            record
                .as_object_mut()
                .ok_or_else(|| invalid("span"))?
                .remove("parentSpanId");
        }
    }
    // The native SDK file uses the zero-based instrumentation SpanKind enum;
    // OTLP reserves zero for unspecified and numbers the same kinds from one.
    record["kind"] = record["kind"]
        .as_u64()
        .filter(|kind| *kind <= 4)
        .ok_or_else(|| invalid("span kind"))?
        .checked_add(1)
        .ok_or_else(|| invalid("span kind"))?
        .into();
    record["startTimeUnixNano"] = timestamp(&record["startTime"])?.into();
    record["endTimeUnixNano"] = timestamp(&record["endTime"])?.into();
    record["attributes"] = attributes(&record["attributes"])?;
    if let Some(events) = record.get_mut("events") {
        for event in events
            .as_array_mut()
            .ok_or_else(|| invalid("span events"))?
        {
            if !event.is_object() {
                return Err(invalid("span event"));
            }
            event["timeUnixNano"] = timestamp(&event["time"])?.into();
            event["attributes"] = attributes(&event["attributes"])?;
        }
    }
    if let Some(links) = record.get_mut("links") {
        for link in links.as_array_mut().ok_or_else(|| invalid("span links"))? {
            if !link.is_object() {
                return Err(invalid("span link"));
            }
            identifier(&link["traceId"], 32, false)?;
            identifier(&link["spanId"], 16, false)?;
            link["attributes"] = attributes(&link["attributes"])?;
        }
    }
    decode(json!({
        "resource":resource,
        "schemaUrl":resource["schemaUrl"].as_str().unwrap_or_default(),
        "scopeSpans":[{
            "scope":scope,
            "schemaUrl":scope["schemaUrl"].as_str().unwrap_or_default(),
            "spans":[record],
        }],
    }))
}

fn number(value: &Value) -> Result<Value, GrpcError> {
    if let Some(number) = value.as_i64() {
        Ok(json!({"asInt":number.to_string()}))
    } else if value.is_f64() {
        Ok(json!({"asDouble":value}))
    } else {
        Err(invalid("metric number"))
    }
}

fn point(record: &Value) -> Result<Value, GrpcError> {
    Ok(json!({
        "attributes":attributes(&record["attributes"])?,
        "startTimeUnixNano":timestamp(&record["startTime"])?,
        "timeUnixNano":timestamp(&record["endTime"])?,
    }))
}

fn histogram_point(record: &Value) -> Result<Value, GrpcError> {
    let value = &record["value"];
    let bounds = value["buckets"]["boundaries"]
        .as_array()
        .ok_or_else(|| invalid("histogram boundaries"))?;
    let counts = value["buckets"]["counts"]
        .as_array()
        .ok_or_else(|| invalid("histogram counts"))?;
    let count = value["count"]
        .as_u64()
        .ok_or_else(|| invalid("histogram count"))?;
    if counts.len() != bounds.len() + 1
        || counts
            .iter()
            .try_fold(0_u64, |sum, value| sum.checked_add(value.as_u64()?))
            != Some(count)
        || bounds.iter().any(|value| value.as_f64().is_none())
        || bounds
            .windows(2)
            .any(|pair| pair[0].as_f64() >= pair[1].as_f64())
    {
        return Err(invalid("histogram buckets"));
    }
    let mut output = point(record)?;
    output["count"] = count.to_string().into();
    output["bucketCounts"] = counts
        .iter()
        .map(|value| {
            value
                .as_u64()
                .map(|count| Value::String(count.to_string()))
                .ok_or_else(|| invalid("histogram counts"))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into();
    output["explicitBounds"] = bounds.clone().into();
    for field in ["sum", "min", "max"] {
        if let Some(value) = value.get(field) {
            if !value.is_number() {
                return Err(invalid("histogram value"));
            }
            output[field] = value.clone();
        }
    }
    Ok(output)
}

pub(super) fn metric(record: &Value) -> Result<ResourceMetrics, GrpcError> {
    let resource = resource(record)?;
    let scope = scope(record)?;
    let name = record["name"]
        .as_str()
        .ok_or_else(|| invalid("metric name"))?;
    let points = record["dataPoints"]
        .as_array()
        .ok_or_else(|| invalid("metric points"))?;
    let histogram = points
        .first()
        .is_some_and(|point| point["value"].is_object());
    let mut metric = json!({
        "name":name,
        "description":record["description"].as_str().unwrap_or_default(),
        "unit":record["unit"].as_str().unwrap_or_default(),
    });
    if histogram {
        let points = points
            .iter()
            .map(histogram_point)
            .collect::<Result<Vec<_>, _>>()?;
        metric["histogram"] = json!({"dataPoints":points,"aggregationTemporality":2});
    } else {
        // The CLI's native exporter emits cumulative histogram aggregations and
        // counters, omitting instrument kind and temporality fields. Limit
        // numeric interpretation to the current CLI's defined counter names.
        if !name.starts_with("gen_ai.client.inference.usage.")
            && !matches!(
                name,
                "github.copilot.tool.call.count"
                    | "github.copilot.mcp.server.connection.count"
                    | "github.copilot.sandbox.operation.count"
                    | "github.copilot.code.lines_added"
                    | "github.copilot.code.lines_removed"
                    | "github.copilot.changeboarding.model_nudge.count"
            )
        {
            return Err(invalid("numeric metric instrument"));
        }
        let points = points
            .iter()
            .map(|record| {
                let mut output = point(record)?;
                let value = number(&record["value"])?;
                output
                    .as_object_mut()
                    .ok_or_else(|| invalid("metric point"))?
                    .extend(
                        value
                            .as_object()
                            .ok_or_else(|| invalid("metric number"))?
                            .clone(),
                    );
                Ok(output)
            })
            .collect::<Result<Vec<_>, GrpcError>>()?;
        metric["sum"] = json!({"dataPoints":points,"aggregationTemporality":2,"isMonotonic":true});
    }
    decode(json!({
        "resource":resource,
        "schemaUrl":resource["schemaUrl"].as_str().unwrap_or_default(),
        "scopeMetrics":[{
            "scope":scope,
            "schemaUrl":scope["schemaUrl"].as_str().unwrap_or_default(),
            "metrics":[metric],
        }],
    }))
}
