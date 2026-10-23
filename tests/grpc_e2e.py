#!/usr/bin/env python3
"""Verify OTLP/gRPC through the real SDK with isolated local test services.

The collector uses generated OTLP services with gRPC Python. Inference is the
existing fixture; no external key is needed.
"""

from __future__ import annotations

import argparse
import json
import tempfile
import threading
import uuid
from concurrent.futures import ThreadPoolExecutor
from http.server import ThreadingHTTPServer
from pathlib import Path

import grpc
from opentelemetry.proto.collector.metrics.v1 import metrics_service_pb2, metrics_service_pb2_grpc
from opentelemetry.proto.collector.trace.v1 import trace_service_pb2, trace_service_pb2_grpc

from acceptance_support import Application, debug_binary
from credential_checks import scan_for_key
from inference_fixture import FIXTURE_KEY, Fixture, handler_for, protobuf_span_names, require
from remote_protocol import StdioServer, private_environment

MAX_MESSAGE = 16 * 1024 * 1024


class TraceService(trace_service_pb2_grpc.TraceServiceServicer):
    def __init__(self, receiver):
        self.receiver = receiver

    def Export(self, request, context):
        self.receiver.export("traces", request, context)
        return trace_service_pb2.ExportTraceServiceResponse()


class MetricsService(metrics_service_pb2_grpc.MetricsServiceServicer):
    def __init__(self, receiver):
        self.receiver = receiver

    def Export(self, request, context):
        self.receiver.export("metrics", request, context)
        return metrics_service_pb2.ExportMetricsServiceResponse()


class Receiver:
    """A real OTLP service, with one deliberate transient collector failure."""

    def __init__(self):
        self.lock = threading.Lock()
        self.exports = []
        self.attempts = 0
        self.failed_payload = None
        self.errors = []
        self.executor = ThreadPoolExecutor(max_workers=4)
        self.server = grpc.server(self.executor, options=[
            ("grpc.max_receive_message_length", MAX_MESSAGE),
        ])
        trace_service_pb2_grpc.add_TraceServiceServicer_to_server(TraceService(self), self.server)
        metrics_service_pb2_grpc.add_MetricsServiceServicer_to_server(MetricsService(self), self.server)
        port = self.server.add_insecure_port("127.0.0.1:0")
        require(port != 0, "Could not bind the local gRPC collector")
        self.endpoint = f"http://127.0.0.1:{port}"
        self.server.start()

    def export(self, kind, request, context):
        payload = request.SerializeToString(deterministic=True)
        try:
            if kind == "traces":
                names = protobuf_span_names(payload)
            else:
                metrics = [metric for resource in request.resource_metrics
                           for scope in resource.scope_metrics for metric in scope.metrics]
                require(all(metric.WhichOneof("data") is not None for metric in metrics),
                        "The gRPC export contained an unrecognized OTLP metric")
                names = [metric.name for metric in metrics]
            require(bool(names) and all(names), "The gRPC export contained no named OTLP records")
        except (AssertionError, ValueError) as error:
            with self.lock:
                self.errors.append(str(error))
            context.abort(grpc.StatusCode.INVALID_ARGUMENT, "Invalid fixture telemetry")
        with self.lock:
            self.attempts += 1
            retry = self.failed_payload is None
            if retry:
                self.failed_payload = payload
            else:
                self.exports.append({"kind": kind, "names": names, "payload": payload})
        if retry:
            context.abort(grpc.StatusCode.UNAVAILABLE, "Retry this fixture export")

    def close(self):
        self.server.stop(0).wait()
        self.executor.shutdown(wait=True)


def telemetry(endpoint, capture=True, enabled=True):
    return {"enabled": enabled, "endpoint": endpoint, "protocol": "grpc", "captureContent": capture}


def check_local(binary, root, inference, endpoint, fixture, marker, metadata_marker):
    root.mkdir()
    home = root / "home"
    home.mkdir()
    environment = private_environment(home)
    app = Application(binary, root, env=environment)
    sessions = []
    try:
        app.command("save_preferences", telemetry=telemetry(endpoint), cliPath=None)
        require(app.command("snapshot")["settings"]["telemetry"]["protocol"] == "grpc",
                "The application did not save the gRPC protocol")
        provider = app.command("save_provider", provider={
            "id": "", "name": "gRPC acceptance", "protocol": "openai-completions",
            "baseUrl": inference + "/v1", "models": [{
                "id": "gpt-4.1", "name": "Acceptance model", "contextWindow": 128000,
                "maxOutputTokens": 4096,
            }],
        }, apiKey=FIXTURE_KEY)
        app.command("save_title_model", titleModel={"providerId": provider["id"], "modelId": "gpt-4.1"})
        project = app.command("create_project", name="gRPC acceptance", path=str(fixture.workspace),
                              connection={"kind": "local"})
        for project_id, prompt in (
            (None, f"fixture:chat {marker} Chat gRPC export acceptance."),
            (project["id"], f"fixture:work-create {marker} Work gRPC export acceptance."),
        ):
            session = app.command("create_session", projectId=project_id,
                                  providerId=provider["id"], modelId="gpt-4.1")
            sessions.append(session["id"])
            events = app.turn(session, prompt)
            app.wait_title(session["id"], events)
            if project_id:
                app.turn(session, f"fixture:work-edit {marker} Replace the file content using the edit tool.")
        require((fixture.workspace / "verified.txt").read_text() == "verified by SDK\n",
                "The Work gRPC case did not execute the real SDK create and edit tools")
        app.command("save_preferences", telemetry=telemetry(endpoint, capture=False), cliPath=None)
        session = app.command("create_session", projectId=None,
                              providerId=provider["id"], modelId="gpt-4.1")
        sessions.append(session["id"])
        events = app.turn(session, f"fixture:chat {metadata_marker} Keep content out of telemetry.")
        app.wait_title(session["id"], events)
        app.command("save_preferences", telemetry=telemetry(endpoint, capture=False, enabled=False), cliPath=None)
    finally:
        app.close()
    restarted = Application(binary, root, env=environment)
    try:
        require(restarted.command("snapshot")["settings"]["telemetry"]["protocol"] == "grpc",
                "The saved gRPC protocol did not survive application restart")
    finally:
        restarted.close()
    require(not scan_for_key(root, FIXTURE_KEY, allow_local_vault=True),
            "The provider credential leaked during local gRPC export")
    return sessions


def check_remote(binary, root, inference, endpoint, marker):
    root.mkdir()
    session_id = str(uuid.uuid4())
    options = {
        "sessionId": session_id, "mode": {"kind": "chat"}, "permissionMode": "ask",
        "provider": {"protocol": "openai-completions", "baseUrl": inference + "/v1", "apiKey": FIXTURE_KEY},
        "model": {"id": "gpt-4.1", "contextWindow": 128000, "maxOutputTokens": 4096},
        "systemPrompt": "Use the requested acceptance response.",
    }
    server = StdioServer(binary, root)
    try:
        server.command("start_agent", options=options, resume=False, telemetry=telemetry(endpoint))
        server.turn(session_id, f"fixture:chat {marker} Remote gRPC export acceptance.")
        server.command("disconnect_agent", sessionId=session_id)
    finally:
        server.close()
    require(not scan_for_key(root, FIXTURE_KEY), "The remote gRPC case persisted the provider credential")
    return session_id


def check_exports(receiver, sessions, marker, metadata_marker):
    require(not receiver.errors, f"The gRPC collector rejected telemetry: {receiver.errors}")
    traces = [batch for batch in receiver.exports if batch["kind"] == "traces"]
    metrics = [batch for batch in receiver.exports if batch["kind"] == "metrics"]
    require(bool(traces), "The collector received no gRPC trace exports")
    require(bool(metrics), "The collector received no gRPC metric exports")
    metric_names = {name for batch in metrics for name in batch["names"]}
    require({"gen_ai.client.operation.duration", "gen_ai.client.inference.usage.input_tokens",
             "github.copilot.tool.call.count", "github.copilot.code.lines_added",
             "github.copilot.code.lines_removed"} <= metric_names,
            "Native inference, tool, or file edit gRPC metrics were lost")
    payloads = b"".join(batch["payload"] for batch in receiver.exports)
    require(all(session.encode() in payloads for session in sessions),
            "Local Chat, Work, metadata-only, or remote session telemetry is missing")
    require(marker.encode() in payloads, "Enabled gRPC content capture omitted conversation content")
    require(metadata_marker.encode() not in payloads, "Disabled gRPC content capture exported conversation text")
    require(FIXTURE_KEY.encode() not in payloads, "The gRPC exports contained the provider credential")
    require(any(batch["payload"] == receiver.failed_payload for batch in receiver.exports),
            "The UNAVAILABLE gRPC export was not retried without losing its payload")
    return {"traceBatches": len(traces), "spans": sum(len(batch["names"]) for batch in traces),
            "metricBatches": len(metrics), "metricNames": sorted(metric_names),
            "attempts": receiver.attempts, "unavailableRetried": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=debug_binary("tandem-dev-server"))
    parser.add_argument("--remote-server", type=Path, default=debug_binary("tandem-server"))
    arguments = parser.parse_args()
    require(arguments.server.is_file() and arguments.remote_server.is_file(), "Build both application servers first")
    marker = "TANDEM_GRPC_ACCEPTANCE_" + uuid.uuid4().hex[:12]
    metadata_marker = "TANDEM_GRPC_METADATA_ONLY_" + uuid.uuid4().hex[:12]
    with tempfile.TemporaryDirectory(prefix="tandem-grpc-e2e-") as temporary:
        root = Path(temporary)
        workspace = root / "project"
        workspace.mkdir()
        fixture = Fixture(workspace)
        inference = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(fixture))
        threading.Thread(target=inference.serve_forever, daemon=True).start()
        inference_url = f"http://127.0.0.1:{inference.server_port}"
        receiver = Receiver()
        try:
            sessions = check_local(arguments.server.resolve(), root / "local", inference_url,
                                   receiver.endpoint, fixture, marker, metadata_marker)
            sessions.append(check_remote(arguments.remote_server.resolve(), root / "remote", inference_url,
                                         receiver.endpoint, marker))
            result = check_exports(receiver, sessions, marker, metadata_marker)
            print(json.dumps({"receiver": "local HTTP/2 gRPC test service", "marker": marker,
                              "sessions": sessions, "inferenceRequests": len(fixture.requests), **result}, indent=2))
        finally:
            inference.shutdown()
            inference.server_close()
            receiver.close()


if __name__ == "__main__":
    main()
