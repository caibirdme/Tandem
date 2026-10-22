use std::{
    convert::Infallible,
    sync::atomic::AtomicUsize,
    task::{Context, Poll},
};

use opentelemetry_proto::tonic::{
    collector::{
        metrics::v1::{
            ExportMetricsPartialSuccess, ExportMetricsServiceResponse,
            metrics_service_server::{MetricsService, MetricsServiceServer},
        },
        trace::v1::{
            ExportTracePartialSuccess, ExportTraceServiceResponse,
            trace_service_server::{TraceService, TraceServiceServer},
        },
    },
    common::v1::any_value,
    metrics::v1::metric,
};
use serde_json::{Value, json};
use tokio::{io::AsyncWriteExt, net::TcpListener, sync::oneshot};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{
    Request, Response, Status,
    body::Body,
    codegen::{BoxFuture, Service, http},
    transport::Server,
};

use super::*;
use crate::TelemetryProtocol;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
type CollectorTask = JoinHandle<std::result::Result<(), tonic::transport::Error>>;

fn span_record() -> Value {
    json!({
        "type":"span",
        "traceId":"0123456789abcdef0123456789abcdef",
        "spanId":"0123456789abcdef",
        "parentSpanId":"fedcba9876543210",
        "name":"chat fixture","kind":2,
        "startTime":[1_700_000_000,100],"endTime":[1_700_000_001,200],
        "attributes":{"gen_ai.conversation.id":"fixture-session","bool":true,"integer":42,"double":1.25,"array":["x",false]},
        "status":{"code":2,"message":"synthetic model failure"},
        "events":[{"name":"fixture event","time":[1_700_000_000,150],"attributes":{"event.key":"event value"}}],
        "resource":{"attributes":{"service.name":"native-service"},"schemaUrl":"https://example.com/resource-schema"},
        "instrumentationScope":{"name":"tandem","version":"1.0","schemaUrl":"https://example.com/scope-schema"},
    })
}

fn metric_record(histogram: bool) -> Value {
    let mut record = span_record();
    record["type"] = "metric".into();
    record["name"] = "gen_ai.client.inference.usage.input_tokens".into();
    record["description"] = "Synthetic metric".into();
    record["unit"] = "{token}".into();
    record["dataPoints"] = json!([{
        "attributes":{"model":"fixture"},
        "startTime":[1_700_000_000,100],"endTime":[1_700_000_001,200],
        "value":42,
    }]);
    if histogram {
        record["name"] = "gen_ai.client.operation.duration".into();
        record["dataPoints"][0]["value"] = json!({
            "buckets":{"boundaries":[1.0,2.0],"counts":[1,1,0]},
            "count":2,"sum":2.5,"min":0.5,"max":2.0,
        });
    }
    record
}

fn settings(endpoint: String) -> TelemetrySettings {
    TelemetrySettings {
        endpoint,
        protocol: TelemetryProtocol::Grpc,
        capture_content: true,
        agentlens: None,
    }
}

#[test]
fn native_spans_preserve_identifiers_timestamps_scope_resource_status_and_events() -> TestResult {
    let output = transform::span(span_record())?;
    assert_eq!(output.schema_url, "https://example.com/resource-schema");
    let resource = output.resource.ok_or("resource missing")?;
    assert_eq!(resource.attributes[0].key, "service.name");
    let scope = &output.scope_spans[0];
    assert_eq!(scope.schema_url, "https://example.com/scope-schema");
    assert_eq!(scope.scope.as_ref().ok_or("scope missing")?.name, "tandem");
    let span = &scope.spans[0];
    assert_eq!(
        span.trace_id,
        vec![
            1, 35, 69, 103, 137, 171, 205, 239, 1, 35, 69, 103, 137, 171, 205, 239
        ]
    );
    assert_eq!(
        span.parent_span_id,
        vec![254, 220, 186, 152, 118, 84, 50, 16]
    );
    assert_eq!(span.kind, 3);
    assert_eq!(span.start_time_unix_nano, 1_700_000_000_000_000_100);
    assert_eq!(span.end_time_unix_nano, 1_700_000_001_000_000_200);
    assert_eq!(span.status.as_ref().ok_or("status missing")?.code, 2);
    assert_eq!(
        span.status.as_ref().ok_or("status missing")?.message,
        "synthetic model failure"
    );
    assert_eq!(span.events[0].time_unix_nano, 1_700_000_000_000_000_150);
    assert!(span.attributes.iter().any(|attribute| {
        attribute.key == "integer"
            && attribute
                .value
                .as_ref()
                .and_then(|value| value.value.as_ref())
                == Some(&any_value::Value::IntValue(42))
    }));
    Ok(())
}

#[test]
fn native_histograms_and_token_counters_preserve_aggregations() -> TestResult {
    let output = transform::metric(&metric_record(true))?;
    let metric::Data::Histogram(histogram) = output.scope_metrics[0].metrics[0]
        .data
        .as_ref()
        .ok_or("histogram missing")?
    else {
        return Err("expected histogram".into());
    };
    assert_eq!(histogram.aggregation_temporality, 2);
    assert_eq!(histogram.data_points[0].count, 2);
    assert_eq!(histogram.data_points[0].bucket_counts, [1, 1, 0]);
    assert_eq!(histogram.data_points[0].explicit_bounds, [1.0, 2.0]);
    assert_eq!(histogram.data_points[0].sum, Some(2.5));
    assert_eq!(histogram.data_points[0].min, Some(0.5));
    assert_eq!(histogram.data_points[0].max, Some(2.0));
    let output = transform::metric(&metric_record(false))?;
    let metric::Data::Sum(sum) = output.scope_metrics[0].metrics[0]
        .data
        .as_ref()
        .ok_or("counter missing")?
    else {
        return Err("expected sum".into());
    };
    assert_eq!(sum.aggregation_temporality, 2);
    assert!(sum.is_monotonic);
    assert_eq!(
        sum.data_points[0].start_time_unix_nano,
        1_700_000_000_000_000_100
    );
    Ok(())
}

#[test]
fn invalid_native_fields_and_unsupported_numeric_instruments_fail_explicitly() {
    for (field, value) in [
        ("traceId", json!("bad")),
        ("spanId", json!("0000000000000000")),
        ("startTime", json!([0, 1_000_000_000])),
        ("endTime", json!([u64::MAX, 0])),
        ("kind", json!(8)),
        ("attributes", json!([])),
        ("resource", json!(42)),
        ("instrumentationScope", json!("bad")),
        ("events", json!([null])),
        ("links", json!([[]])),
        ("parentSpanId", json!("0")),
    ] {
        let mut record = span_record();
        record[field] = value;
        assert!(transform::span(record).is_err(), "accepted invalid {field}");
    }
    let mut record = metric_record(false);
    record["name"] = "unknown.instrument".into();
    assert!(transform::metric(&record).is_err());
    let mut record = metric_record(true);
    record["dataPoints"][0]["value"]["count"] = 99.into();
    assert!(transform::metric(&record).is_err());
}

#[test]
fn current_cli_tool_edit_mcp_sandbox_and_changeboarding_counters_are_monotonic_sums() -> TestResult
{
    for name in [
        "github.copilot.tool.call.count",
        "github.copilot.mcp.server.connection.count",
        "github.copilot.sandbox.operation.count",
        "github.copilot.code.lines_added",
        "github.copilot.code.lines_removed",
        "github.copilot.changeboarding.model_nudge.count",
    ] {
        let mut record = metric_record(false);
        record["name"] = name.into();
        let output = transform::metric(&record)?;
        let metric = &output.scope_metrics[0].metrics[0];
        assert_eq!(metric.name, name);
        let Some(metric::Data::Sum(sum)) = &metric.data else {
            return Err("expected a monotonic counter".into());
        };
        assert!(sum.is_monotonic);
        assert_eq!(sum.aggregation_temporality, 2);
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Behavior {
    Success,
    Retry,
    RejectSpans,
    RejectMetrics,
    Deny,
}

#[derive(Clone)]
struct Collector {
    behavior: Behavior,
    traces: Arc<AtomicUsize>,
    metrics: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct Services {
    traces: TraceServiceServer<Collector>,
    metrics: MetricsServiceServer<Collector>,
}

impl Service<http::Request<Body>> for Services {
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(
        &mut self,
        _context: &mut Context<'_>,
    ) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<Body>) -> Self::Future {
        if request.uri().path().contains("MetricsService") {
            self.metrics.call(request)
        } else {
            self.traces.call(request)
        }
    }
}

#[tonic::async_trait]
impl TraceService for Collector {
    async fn export(
        &self,
        request: Request<ExportTraceServiceRequest>,
    ) -> std::result::Result<Response<ExportTraceServiceResponse>, Status> {
        assert_ne!(request.into_inner().resource_spans.len(), 0);
        let count = self.traces.fetch_add(1, Ordering::SeqCst);
        match self.behavior {
            Behavior::Retry if count == 0 => {
                return Err(Status::unavailable("private receiver detail"));
            }
            Behavior::Deny => return Err(Status::permission_denied("private receiver detail")),
            _ => {}
        }
        Ok(Response::new(ExportTraceServiceResponse {
            partial_success: matches!(self.behavior, Behavior::RejectSpans).then(|| {
                ExportTracePartialSuccess {
                    rejected_spans: 1,
                    error_message: "private receiver detail".into(),
                }
            }),
        }))
    }
}

#[tonic::async_trait]
impl MetricsService for Collector {
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> std::result::Result<Response<ExportMetricsServiceResponse>, Status> {
        assert_ne!(request.into_inner().resource_metrics.len(), 0);
        let count = self.metrics.fetch_add(1, Ordering::SeqCst);
        if matches!(self.behavior, Behavior::Retry) && count == 0 {
            return Err(Status::unavailable("private receiver detail"));
        }
        Ok(Response::new(ExportMetricsServiceResponse {
            partial_success: matches!(self.behavior, Behavior::RejectMetrics).then(|| {
                ExportMetricsPartialSuccess {
                    rejected_data_points: 2,
                    error_message: "private receiver detail".into(),
                }
            }),
        }))
    }
}

async fn collector(
    behavior: Behavior,
) -> std::result::Result<
    (String, Collector, oneshot::Sender<()>, CollectorTask),
    Box<dyn std::error::Error>,
> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    let collector = Collector {
        behavior,
        traces: Arc::new(AtomicUsize::new(0)),
        metrics: Arc::new(AtomicUsize::new(0)),
    };
    let service = collector.clone();
    let (stop, receiver) = oneshot::channel();
    let task = tokio::spawn(async move {
        Server::builder()
            .serve_with_incoming_shutdown(
                Services {
                    traces: TraceServiceServer::new(service.clone()),
                    metrics: MetricsServiceServer::new(service),
                },
                TcpListenerStream::new(listener),
                async {
                    let _ = receiver.await;
                },
            )
            .await
    });
    Ok((endpoint, collector, stop, task))
}

#[tokio::test]
async fn grpc_exports_both_signals_retries_transient_failures_and_sanitizes_rejections()
-> TestResult {
    for behavior in [
        Behavior::Success,
        Behavior::Retry,
        Behavior::RejectSpans,
        Behavior::RejectMetrics,
        Behavior::Deny,
    ] {
        let (endpoint, collector, stop, task) = collector(behavior).await?;
        let mut sender = Sender::new(&settings(endpoint))?;
        let mut batch = Batch::default();
        batch.push(span_record())?;
        batch.push(metric_record(true))?;
        let result = sender.send(&batch).await;
        match behavior {
            Behavior::Success => {
                result?;
                assert_eq!(collector.traces.load(Ordering::SeqCst), 1);
                assert_eq!(collector.metrics.load(Ordering::SeqCst), 1);
            }
            Behavior::Retry => {
                result?;
                assert_eq!(collector.traces.load(Ordering::SeqCst), 2);
                assert_eq!(collector.metrics.load(Ordering::SeqCst), 2);
            }
            _ => {
                let Err(error) = result else {
                    return Err("collector rejection missing".into());
                };
                assert!(!format!("{error:?} {error}").contains("private receiver detail"));
                assert!(error.is_invalid_input());
                if matches!(behavior, Behavior::RejectMetrics) {
                    assert!(matches!(error, GrpcError::Rejected { count: 2, .. }));
                }
            }
        }
        drop(sender);
        let _ = stop.send(());
        task.await??;
    }
    Ok(())
}

#[tokio::test]
async fn private_spool_flushes_both_signals_and_is_removed() -> TestResult {
    let (endpoint, collector, stop, task) = collector(Behavior::Success).await?;
    let directory = tempfile::tempdir()?;
    let exporter = GrpcExport::start(&settings(endpoint), directory.path()).await?;
    let configuration = exporter.configuration();
    assert_eq!(configuration.exporter_type, Some(OtelExporterType::File));
    assert_eq!(configuration.capture_content, Some(true));
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(configuration.file_path.ok_or("spool path missing")?)
        .await?;
    for record in [span_record(), metric_record(true), metric_record(false)] {
        file.write_all(format!("{record}\n").as_bytes()).await?;
    }
    file.flush().await?;
    drop(file);
    exporter.finish().await?;
    assert!(collector.traces.load(Ordering::SeqCst) > 0);
    assert!(collector.metrics.load(Ordering::SeqCst) > 0);
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 0);
    let _ = stop.send(());
    task.await??;
    Ok(())
}

#[tokio::test]
async fn malformed_private_spool_fails_without_retaining_content() -> TestResult {
    let (endpoint, _, stop, task) = collector(Behavior::Success).await?;
    let directory = tempfile::tempdir()?;
    let exporter = GrpcExport::start(&settings(endpoint), directory.path()).await?;
    tokio::fs::write(&exporter.path, b"private synthetic truncated record").await?;
    let Err(error) = exporter.finish().await else {
        return Err("incomplete record rejection missing".into());
    };
    assert!(!format!("{error:?} {error}").contains("private synthetic"));
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 0);
    let _ = stop.send(());
    task.await??;
    Ok(())
}

#[test]
fn queued_bytes_are_bounded_and_released_after_send() -> TestResult {
    let (exports, mut receiver) = mpsc::channel(1);
    let outstanding = Arc::new(AtomicUsize::new(MAX_PENDING_BYTES));
    let mut batch = Batch::default();
    batch.push(span_record())?;
    assert!(matches!(
        enqueue(&mut batch, &exports, &outstanding),
        Err(TelemetryError::QueueLimit)
    ));
    outstanding.store(0, Ordering::Release);
    enqueue(&mut batch, &exports, &outstanding)?;
    assert!(outstanding.load(Ordering::Acquire) > 0);
    drop(receiver.try_recv()?);
    assert_eq!(outstanding.load(Ordering::Acquire), 0);
    Ok(())
}
