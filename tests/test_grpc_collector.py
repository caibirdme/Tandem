"""Verify that acceptance telemetry uses real OTLP routing and schema decoding."""

import unittest

import grpc
from google.protobuf.message import DecodeError
from opentelemetry.proto.collector.metrics.v1 import metrics_service_pb2, metrics_service_pb2_grpc
from opentelemetry.proto.collector.trace.v1 import trace_service_pb2, trace_service_pb2_grpc

from grpc_e2e import Receiver
from inference_fixture import protobuf_span_names


def trace_request():
    request = trace_service_pb2.ExportTraceServiceRequest()
    span = request.resource_spans.add().scope_spans.add().spans.add()
    span.trace_id = b"t" * 16
    span.span_id = b"s" * 8
    span.name = "fixture span"
    return request


class CollectorTests(unittest.TestCase):
    def setUp(self):
        self.receiver = Receiver()
        self.channel = grpc.insecure_channel(self.receiver.endpoint.removeprefix("http://"))
        self.addCleanup(self.receiver.close)
        self.addCleanup(self.channel.close)

    def test_generated_services_retry_and_keep_both_signals(self):
        traces = trace_service_pb2_grpc.TraceServiceStub(self.channel)
        request = trace_request()
        with self.assertRaises(grpc.RpcError) as caught:
            traces.Export(request, timeout=5)
        self.assertEqual(caught.exception.code(), grpc.StatusCode.UNAVAILABLE)
        traces.Export(request, timeout=5)
        metrics = metrics_service_pb2.ExportMetricsServiceRequest()
        metric = metrics.resource_metrics.add().scope_metrics.add().metrics.add()
        metric.name = "fixture metric"
        metric.gauge.data_points.add().as_int = 42
        metrics_service_pb2_grpc.MetricsServiceStub(self.channel).Export(metrics, timeout=5)
        self.assertEqual(self.receiver.attempts, 3)
        self.assertEqual([batch["kind"] for batch in self.receiver.exports], ["traces", "metrics"])
        self.assertEqual(self.receiver.exports[0]["payload"], self.receiver.failed_payload)
        self.assertEqual(self.receiver.exports[1]["names"], ["fixture metric"])
        self.assertEqual(self.receiver.errors, [])

    def test_wrong_rpc_method_is_rejected_even_with_valid_trace_bytes(self):
        export = self.channel.unary_unary("/not.otlp.TraceService/Export")
        with self.assertRaises(grpc.RpcError) as caught:
            export(trace_request().SerializeToString(), timeout=5)
        self.assertEqual(caught.exception.code(), grpc.StatusCode.UNIMPLEMENTED)
        self.assertEqual(self.receiver.attempts, 0)

    def test_invalid_trace_ids_are_rejected_by_service(self):
        request = trace_request()
        request.resource_spans[0].scope_spans[0].spans[0].trace_id = b"short"
        with self.assertRaises(grpc.RpcError) as caught:
            trace_service_pb2_grpc.TraceServiceStub(self.channel).Export(request, timeout=5)
        self.assertEqual(caught.exception.code(), grpc.StatusCode.INVALID_ARGUMENT)
        self.assertEqual(self.receiver.attempts, 0)
        self.assertEqual(len(self.receiver.errors), 1)

    def test_generated_parser_rejects_truncated_protobuf(self):
        with self.assertRaises(DecodeError):
            protobuf_span_names(b"\x0a\x80")
        self.assertEqual(protobuf_span_names(trace_request().SerializeToString()), ["fixture span"])


if __name__ == "__main__":
    unittest.main()
