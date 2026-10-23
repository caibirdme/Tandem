#!/usr/bin/env python3
"""Verify OTLP/gRPC through the real SDK with isolated local test services.

The collector implements the HTTP/2 framing needed for unary gRPC in Python's
standard library. Inference is the existing fixture; no external key is needed.
"""

from __future__ import annotations

import argparse
import json
import socket
import socketserver
import struct
import tempfile
import threading
import uuid
from http.server import ThreadingHTTPServer
from pathlib import Path

from acceptance_support import Application, debug_binary
from credential_checks import scan_for_key
from inference_fixture import FIXTURE_KEY, Fixture, handler_for, protobuf_fields, protobuf_span_names, require
from remote_protocol import StdioServer, private_environment

PREFACE = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"
MAX_MESSAGE = 16 * 1024 * 1024


def frame(kind, flags, stream, payload=b""):
    return len(payload).to_bytes(3, "big") + bytes((kind, flags)) + struct.pack("!I", stream) + payload


def header(name, value):
    # Literal HPACK header without indexing or Huffman coding. All fixture
    # response names and values fit HPACK's one-byte length representation.
    return b"\x00" + bytes((len(name),)) + name + bytes((len(value),)) + value


def signal_names(payload):
    items = [item for number, resource in protobuf_fields(payload) if number == 1
             for number, scope in protobuf_fields(resource) if number == 2
             for number, item in protobuf_fields(scope) if number == 2]
    require(bool(items), "The gRPC export contained no OTLP spans or metrics")
    first = dict(protobuf_fields(items[0]))
    if len(first.get(1, b"")) == 16 and len(first.get(2, b"")) == 8:
        return "traces", protobuf_span_names(payload)
    names = []
    for item in items:
        fields = dict(protobuf_fields(item))
        require(any(number in fields for number in (5, 7, 9, 10, 11)),
                "The gRPC export contained an unrecognized OTLP metric")
        names.append(fields.get(1, b"").decode())
    require(all(names), "An exported gRPC metric has no name")
    return "metrics", names


class Receiver(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True
    block_on_close = False

    def __init__(self):
        self.lock = threading.Lock()
        self.exports = []
        self.attempts = 0
        self.failed_payload = None
        self.errors = []
        self.connections = set()
        super().__init__(("127.0.0.1", 0), CollectorConnection)
        self.thread = threading.Thread(target=self.serve_forever, daemon=True)
        self.thread.start()

    @property
    def endpoint(self):
        return f"http://127.0.0.1:{self.server_address[1]}"

    def export(self, payload):
        kind, names = signal_names(payload)
        with self.lock:
            self.attempts += 1
            if self.failed_payload is None:
                self.failed_payload = payload
                return b"14"
            self.exports.append({"kind": kind, "names": names, "payload": payload})
        return b"0"

    def close(self):
        self.shutdown()
        with self.lock:
            connections = list(self.connections)
        for connection in connections:
            try:
                connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            connection.close()
        self.server_close()
        self.thread.join(timeout=2)


class CollectorConnection(socketserver.BaseRequestHandler):
    def read_exact(self, size):
        data = bytearray()
        while len(data) < size:
            chunk = self.request.recv(size - len(data))
            if not chunk and not data:
                return None
            require(bool(chunk), "The gRPC client truncated an HTTP/2 frame")
            data.extend(chunk)
        return bytes(data)

    def handle(self):
        receiver = self.server
        with receiver.lock:
            receiver.connections.add(self.request)
        try:
            require(self.read_exact(len(PREFACE)) == PREFACE, "The exporter did not use HTTP/2")
            # Increase both flow-control windows for the bounded OTLP request.
            self.request.sendall(frame(4, 0, 0, struct.pack("!HI", 4, MAX_MESSAGE))
                                 + frame(8, 0, 0, struct.pack("!I", 0x7FFFFFFF - 65535)))
            streams = {}
            while True:
                raw = self.read_exact(9)
                if raw is None:
                    return
                size, kind, flags = int.from_bytes(raw[:3], "big"), raw[3], raw[4]
                stream = int.from_bytes(raw[5:], "big") & 0x7FFFFFFF
                require(size <= MAX_MESSAGE, "The gRPC client exceeded the HTTP/2 frame limit")
                payload = self.read_exact(size) if size else b""
                require(payload is not None, "The gRPC client omitted an HTTP/2 payload")
                if kind == 4 and not flags & 1:
                    self.request.sendall(frame(4, 1, 0))
                elif kind == 6 and not flags & 1:
                    self.request.sendall(frame(6, 1, 0, payload))
                elif kind == 1:
                    # HPACK request headers are opaque to this transport fixture.
                    # OTLP protobuf structure distinguishes the two signal types.
                    streams.setdefault(stream, bytearray())
                elif kind == 0:
                    require(stream in streams, "The gRPC request omitted HTTP/2 headers")
                    body = payload[1:len(payload) - payload[0]] if flags & 8 else payload
                    streams[stream].extend(body)
                    require(len(streams[stream]) <= MAX_MESSAGE + 5, "The gRPC message exceeded its limit")
                    if size:
                        window = struct.pack("!I", size)
                        self.request.sendall(frame(8, 0, 0, window) + frame(8, 0, stream, window))
                elif kind == 3:
                    streams.pop(stream, None)
                elif kind == 7:
                    return
                if kind in (0, 1) and flags & 1:
                    message = bytes(streams.pop(stream))
                    require(len(message) >= 5 and message[0] == 0, "The gRPC envelope is invalid or compressed")
                    require(int.from_bytes(message[1:5], "big") == len(message) - 5,
                            "The unary gRPC request length did not match its protobuf message")
                    status = receiver.export(message[5:])
                    self.request.sendall(frame(1, 4, stream, b"\x88" + header(b"content-type", b"application/grpc"))
                                         + frame(0, 0, stream, b"\x00\x00\x00\x00\x00")
                                         + frame(1, 5, stream, header(b"grpc-status", status)))
        except (AssertionError, UnicodeError, ValueError, KeyError) as error:
            with receiver.lock:
                receiver.errors.append(str(error))
        except (ConnectionError, OSError):
            # The application owns and closes its channels during shutdown.
            pass
        finally:
            with receiver.lock:
                receiver.connections.discard(self.request)


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
    require(not receiver.errors, f"The gRPC collector rejected transport frames: {receiver.errors}")
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
