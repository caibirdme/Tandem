#!/usr/bin/env python3
"""Verify AgentLens export with the real SDK and isolated inference services.

The default receiver is a strict local OTLP fixture. --live-endpoint sends only
synthetic acceptance conversations to that receiver and reads its key from stdin.
No credential is accepted in command-line arguments or printed in diagnostics.
"""

from __future__ import annotations

import argparse
import getpass
import json
import sqlite3
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid
from contextlib import closing
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from acceptance_support import Application, debug_binary
from credential_checks import contains_key, scan_for_key
from inference_fixture import FIXTURE_KEY, Fixture, handler_for, is_title_request, require
from remote_protocol import StdioServer, private_environment

TELEMETRY_KEY = "fixture-agentlens-key#tenant#not-a-credential"


def attributes(items):
    return {
        item["key"]: next(iter(item["value"].values()))
        for item in items or []
    }


class Receiver:
    def __init__(self, forced_status=None, response_gate=None):
        self.exports = []
        self.attempts = 0
        self.metrics = 0
        self.failed_export = None
        self.lock = threading.Lock()
        self.forced_status = forced_status
        self.response_gate = response_gate
        self.started = threading.Event()

    def handler(self):
        receiver = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_POST(self):
                raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                with receiver.lock:
                    receiver.attempts += 1
                    attempt = receiver.attempts
                    if self.path.endswith("/v1/metrics"):
                        receiver.metrics += 1
                valid = self.path == "/v1/traces" and self.headers.get_content_type() == "application/json"
                try:
                    body = json.loads(raw)
                    resources = body["resourceSpans"]
                    valid = valid and bool(resources) and all(
                        attributes(resource["resource"]["attributes"]).get("tps.tenant.id") == TELEMETRY_KEY
                        and attributes(resource["resource"]["attributes"]).get("service.name") == "tandem"
                        for resource in resources
                    )
                except (ValueError, KeyError, TypeError):
                    body = {}
                    valid = False
                receiver.started.set()
                if receiver.response_gate:
                    receiver.response_gate.wait(30)
                status = receiver.forced_status or (503 if attempt == 1 else 200 if valid else 400)
                if status == 503:
                    receiver.failed_export = body
                if status == 200:
                    with receiver.lock:
                        receiver.exports.append(body)
                response = b"{}"
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(response)))
                self.end_headers()
                self.wfile.write(response)

        return Handler


def telemetry(endpoint, enabled=True):
    return {
        "enabled": enabled,
        "endpoint": endpoint,
        "protocol": "http/json",
        "captureContent": True,
        "agentlens": {"serviceName": "tandem"},
    }


def assert_private_storage(root, key):
    database = root / "data" / "tandem.sqlite3"
    with closing(sqlite3.connect(database)) as connection:
        row = connection.execute("SELECT secret FROM telemetry_secrets WHERE id=1").fetchone()
        require(row is not None and row[0] == key, "Telemetry credential was not saved separately")
        schema = connection.execute("SELECT type,name,sql FROM sqlite_schema").fetchall()
        require(key not in json.dumps(schema), "Telemetry credential appeared in database schema")
        for kind, table, _sql in schema:
            if kind != "table":
                continue
            quoted = '"' + table.replace('"', '""') + '"'
            cursor = connection.execute(f"SELECT * FROM {quoted}")
            checked = [index for index, column in enumerate(cursor.description)
                       if (table, column[0]) != ("telemetry_secrets", "secret")]
            require(all(key not in str(row[index]) for row in cursor for index in checked),
                    "Telemetry credential appeared outside its dedicated database column")
    database_files = {database, database.with_name(database.name + "-wal"),
                      database.with_name(database.name + "-shm")}
    leaks = [path.relative_to(root).as_posix() for path in root.rglob("*")
             if path.is_file() and path not in database_files and contains_key(path, key.encode())]
    require(not leaks, f"Telemetry credential appeared in runtime files or logs: {leaks}")


def check_local(binary, root, inference, endpoint, fixture, key, marker, metadata_marker=None):
    root.mkdir()
    home = root / "home"
    home.mkdir()
    environment = private_environment(home)
    app = Application(binary, root, env=environment)
    sessions = []
    metadata_session = None
    configured = telemetry(endpoint)
    try:
        app.command("save_preferences", telemetry=configured, cliPath=None, telemetryApiKey=key)
        require(app.command("get_telemetry_api_key") == key, "On-demand telemetry credential lookup failed")
        require(key not in json.dumps(app.command("snapshot")), "Snapshot exposed telemetry credential")
        provider = app.command("save_provider", provider={
            "id": "", "name": "AgentLens acceptance", "protocol": "openai-completions",
            "baseUrl": inference + "/v1",
            "models": [{"id": "gpt-4.1", "name": "Acceptance model", "contextWindow": 128000,
                        "maxOutputTokens": 4096}],
        }, apiKey=FIXTURE_KEY)
        app.command("save_title_model", titleModel={"providerId": provider["id"], "modelId": "gpt-4.1"})
        project = app.command("create_project", name="AgentLens acceptance", path=str(fixture.workspace),
                              connection={"kind": "local"})
        for project_id, prompt in (
            (None, f"fixture:chat {marker} Chat export acceptance."),
            (project["id"], f"fixture:work-create {marker} Work export acceptance."),
        ):
            session = app.command("create_session", projectId=project_id,
                                  providerId=provider["id"], modelId="gpt-4.1")
            sessions.append(session["id"])
            events = app.turn(session, prompt)
            app.wait_title(session["id"], events)
            if project_id:
                for worker in ("first", "second"):
                    (fixture.workspace / f"fleet-{worker}.txt").write_text(f"AgentLens {worker} worker acceptance\n")
                app.turn(session, f"fixture:fleet {marker} Verify nested agent export.", fleet=True)
        require(any(is_title_request(body) for _path, body in fixture.requests),
                "Acceptance did not exercise the excluded title helper")
        if metadata_marker:
            metadata_config = {**configured, "captureContent": False}
            app.command("save_preferences", telemetry=metadata_config, cliPath=None)
            session = app.command("create_session", projectId=None,
                                  providerId=provider["id"], modelId="gpt-4.1")
            metadata_session = session["id"]
            sessions.append(metadata_session)
            events = app.turn(session, f"fixture:chat {metadata_marker} Do not capture this content.")
            app.wait_title(metadata_session, events)
        app.command("save_preferences", telemetry=telemetry(endpoint, False), cliPath=None)
        require(app.command("get_telemetry_api_key") == key, "Saving preferences lost telemetry credential")
    finally:
        app.close()
    assert_private_storage(root, key)
    require(not scan_for_key(root, FIXTURE_KEY, allow_local_vault=True),
            "Provider credential leaked during AgentLens export")
    restarted = Application(binary, root, env=environment)
    try:
        require(restarted.command("get_telemetry_api_key") == key, "Telemetry credential did not survive restart")
        restarted.command("save_preferences", telemetry=telemetry(endpoint, False), cliPath=None,
                          telemetryApiKey="")
        require(restarted.command("get_telemetry_api_key") is None, "Telemetry credential could not be cleared")
    finally:
        restarted.close()
    return sessions, metadata_session


def check_remote(binary, root, inference, endpoint, key, marker):
    root.mkdir()
    session_id = str(uuid.uuid4())
    options = {
        "sessionId": session_id, "mode": {"kind": "chat"}, "permissionMode": "ask",
        "provider": {"protocol": "openai-completions", "baseUrl": inference + "/v1", "apiKey": FIXTURE_KEY},
        "model": {"id": "gpt-4.1", "contextWindow": 128000, "maxOutputTokens": 4096},
        "systemPrompt": "Use the requested acceptance response.",
    }
    configured = telemetry(endpoint)
    configured["agentlens"]["apiKey"] = key
    server = StdioServer(binary, root)
    try:
        server.command("start_agent", options=options, resume=False, telemetry=configured)
        server.turn(session_id, f"fixture:chat {marker} Remote export acceptance.")
        server.command("disconnect_agent", sessionId=session_id)
    finally:
        server.close()
    require(not scan_for_key(root, key), "Remote host persisted the AgentLens credential")
    require(not scan_for_key(root, FIXTURE_KEY), "Remote host persisted the provider credential")
    return session_id


def check_idle_export_failure(binary, root, inference, fixture):
    """A failed idle export must be reported once before explicit runtime retry."""
    root.mkdir()
    home = root / "home"
    home.mkdir()
    gate = threading.Event()
    receiver = Receiver(forced_status=401, response_gate=gate)
    collector = ThreadingHTTPServer(("127.0.0.1", 0), receiver.handler())
    threading.Thread(target=collector.serve_forever, daemon=True).start()
    endpoint = f"http://127.0.0.1:{collector.server_port}"
    app = Application(binary, root, env=private_environment(home))
    try:
        app.command("save_preferences", telemetry=telemetry(endpoint), cliPath=None,
                    telemetryApiKey=TELEMETRY_KEY)
        provider = app.command("save_provider", provider={
            "id":"", "name":"Idle export failure", "protocol":"openai-completions",
            "baseUrl":inference + "/v1", "models":[{
                "id":"gpt-4.1", "name":"Acceptance model", "contextWindow":128000,
                "maxOutputTokens":4096,
            }],
        }, apiKey=FIXTURE_KEY)
        session = app.command("create_session", projectId=None,
                              providerId=provider["id"], modelId="gpt-4.1")
        app.turn(session, "fixture:chat Finish this turn before rejecting its trace export.")
        require(receiver.started.wait(10), "Idle failure receiver did not receive a trace")
        spool = root / "data" / "runtime"
        require(bool(list(spool.glob("agentlens-*"))), "The gated exporter was not running")
        gate.set()
        deadline = time.monotonic() + 15
        while list(spool.glob("agentlens-*")) and time.monotonic() < deadline:
            time.sleep(0.05)
        require(not list(spool.glob("agentlens-*")), "The rejected export did not fail-stop the runtime")
        previous = app.command("messages", sessionId=session["id"])
        before = len(fixture.requests)
        request = urllib.request.Request(app.base + "/api/dispatch", json.dumps({
            "command":"send_message", "payload":{
                "sessionId":session["id"], "content":"fixture:chat Observe the failed idle export.",
                "messageId":str(uuid.uuid4()),
            },
        }).encode(), {"Content-Type":"application/json"})
        try:
            with urllib.request.urlopen(request, timeout=30):
                raise AssertionError("The failed idle export was silently discarded")
        except urllib.error.HTTPError as error:
            failure = json.load(error)
            require(failure.get("code") == "invalid_input", "The receiver rejection lost its typed error code")
        require(len(fixture.requests) == before, "The failed export was hidden by new inference")
        require(app.command("messages", sessionId=session["id"]) == previous,
                "The rejected recovery changed conversation history")
        require(not app.command("snapshot")["runtime"]["available"],
                "The failed export silently started a replacement runtime")
        app.wait_event(session["id"], lambda event: event["kind"] == "error")
        receiver.forced_status = 200
        app.turn(session, "fixture:chat Explicit retry after recovering the telemetry receiver.")
        app.command("save_preferences", telemetry=telemetry(endpoint, False), cliPath=None)
        require(bool(receiver.exports), "Explicit retry did not restore telemetry export")
    finally:
        gate.set()
        app.close()
        collector.shutdown()
        collector.server_close()
    assert_private_storage(root, TELEMETRY_KEY)


def check_exports(receiver, sessions, marker, metadata_session=None, metadata_marker=None):
    def identities(batch):
        return {(span["traceId"], span["spanId"])
                for resource in batch.get("resourceSpans", [])
                for scope in resource.get("scopeSpans", []) for span in scope.get("spans", [])}

    failed = identities(receiver.failed_export or {})
    accepted = set().union(*(identities(batch) for batch in receiver.exports))
    require(bool(failed) and failed <= accepted, "Spans from the transient OTLP failure were not retried")
    require(receiver.metrics == 0, "AgentLens export included independent metrics")
    spans = []
    for batch in receiver.exports:
        for resource in batch["resourceSpans"]:
            for scope in resource.get("scopeSpans", []):
                spans.extend(scope.get("spans", []))
    require(bool(spans), "AgentLens export contained no spans")
    by_identity = {(span["traceId"], span["spanId"]): span for span in spans}
    seen_sessions = set()
    kinds = set()
    missing_initial_subagent_inputs = 0
    for span in spans:
        fields = attributes(span.get("attributes"))
        classification = fields.get("gen_ai.span.kind")
        kinds.add(classification)
        require(classification in {"AGENT", "LLM", "TOOL", "TASK", "CHAIN"},
                "Span did not carry an AgentLens classification")
        root = not span.get("parentSpanId")
        require(span.get("kind") == (2 if root else 1), "Span kind did not match AgentLens entry/child convention")
        session_id = fields.get("gen_ai.session.id")
        require(session_id in sessions, "Unregistered or title session was exported")
        seen_sessions.add(session_id)
        if classification == "LLM":
            require(fields.get("gen_ai.request.model") == "gpt-4.1", "LLM model attribute was lost")
            require(bool(fields.get("gen_ai.system")), "LLM provider attribute was lost")
            if session_id != metadata_session:
                require(bool(fields.get("output.value")), "Captured LLM output was not mapped")
                if not fields.get("input.value"):
                    # CLI 1.0.102 omits the first subagent request's input. Do
                    # not invent a model request from later conversation history.
                    # Its actual instruction must remain in the parent task, and
                    # subsequent requests must carry the native full input.
                    parent = by_identity.get((span["traceId"], span.get("parentSpanId")), {})
                    parent_fields = attributes(parent.get("attributes"))
                    task = by_identity.get((span["traceId"], parent.get("parentSpanId")), {})
                    task_fields = attributes(task.get("attributes"))
                    require(parent_fields.get("gen_ai.span.kind") == "AGENT"
                            and bool(parent.get("parentSpanId"))
                            and task_fields.get("tool.name") == "task"
                            and "fixture:fleet-worker" in str(task_fields.get("input.value")),
                            "Captured model input is missing outside the known initial-subagent SDK limitation")
                    require(any(peer.get("parentSpanId") == span.get("parentSpanId")
                                and peer["traceId"] == span["traceId"]
                                and bool(attributes(peer.get("attributes")).get("input.value"))
                                for peer in spans if peer is not span),
                            "Subagent subsequent model input was not captured")
                    missing_initial_subagent_inputs += 1
            else:
                require(not fields.get("input.value") and not fields.get("output.value"),
                        "Disabled content capture still exported LLM input/output")
        if classification == "TOOL":
            require(bool(fields.get("tool.name")), "Tool name was not mapped")
            require(bool(fields.get("input.value")) and bool(fields.get("output.value")),
                    "Full tool input/output content was not mapped")
    require(seen_sessions == set(sessions), "Chat, Work, or remote conversation export is missing")
    require({"AGENT", "LLM", "TOOL"}.issubset(kinds), "Agent, model, and tool spans were not all exported")
    require(any(attributes(span.get("attributes")).get("gen_ai.span.kind") == "AGENT"
                and bool(span.get("parentSpanId")) for span in spans),
            "Nested Work subagent spans were not exported")
    encoded = json.dumps(receiver.exports)
    require(marker in encoded, "Conversation content was not exported")
    if metadata_marker:
        require(metadata_marker not in encoded, "Disabled content capture exported conversation text")
    require("TANDEM_TITLE_SUMMARY" not in encoded, "Title generation content was exported")
    require(FIXTURE_KEY not in encoded, "Provider credential leaked into telemetry")
    return {"spans": len(spans), "nativeInitialSubagentInputsAbsent": missing_initial_subagent_inputs}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=debug_binary("tandem-dev-server"))
    parser.add_argument("--remote-server", type=Path, default=debug_binary("tandem-server"))
    parser.add_argument("--live-endpoint", help="Send synthetic data to this AgentLens base URL; key is read from stdin")
    arguments = parser.parse_args()
    require(arguments.server.is_file() and arguments.remote_server.is_file(), "Build both application servers first")
    if arguments.live_endpoint:
        key = (getpass.getpass("AgentLens access key: ") if sys.stdin.isatty()
               else sys.stdin.readline()).strip()
    else:
        key = TELEMETRY_KEY
    require(bool(key), "Provide a nonempty AgentLens credential on stdin")
    marker = "TANDEM_AGENTLENS_ACCEPTANCE_" + uuid.uuid4().hex[:12]
    metadata_marker = None if arguments.live_endpoint else "TANDEM_METADATA_ONLY_" + uuid.uuid4().hex[:12]
    with tempfile.TemporaryDirectory(prefix="tandem-agentlens-e2e-") as temporary:
        root = Path(temporary)
        workspace = root / "project"
        workspace.mkdir()
        fixture = Fixture(workspace)
        inference = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(fixture))
        threading.Thread(target=inference.serve_forever, daemon=True).start()
        inference_url = f"http://127.0.0.1:{inference.server_port}"
        receiver = Receiver()
        collector = ThreadingHTTPServer(("127.0.0.1", 0), receiver.handler())
        threading.Thread(target=collector.serve_forever, daemon=True).start()
        endpoint = arguments.live_endpoint or f"http://127.0.0.1:{collector.server_port}"
        try:
            sessions, metadata_session = check_local(arguments.server.resolve(), root / "local", inference_url,
                                                     endpoint, fixture, key, marker, metadata_marker)
            sessions.append(check_remote(arguments.remote_server.resolve(), root / "remote",
                                         inference_url, endpoint, key, marker))
            result = {"marker": marker, "sessions": sessions, "inferenceRequests": len(fixture.requests),
                      "receiver": "live AgentLens" if arguments.live_endpoint else "local test service"}
            if not arguments.live_endpoint:
                check_idle_export_failure(arguments.server.resolve(), root / "idle-failure",
                                          inference_url, fixture)
                result["idleFailureRecovery"] = True
                result["inferenceRequests"] = len(fixture.requests)
                try:
                    result.update(check_exports(receiver, sessions, marker, metadata_session, metadata_marker))
                except AssertionError:
                    evidence = json.loads(json.dumps(receiver.exports))
                    for batch in evidence:
                        for resource in batch["resourceSpans"]:
                            for item in resource["resource"]["attributes"]:
                                if item["key"] == "tps.tenant.id":
                                    item["value"] = {"stringValue": "[redacted]"}
                    output = Path(__file__).resolve().parents[1] / "test-results" / "agentlens-export.json"
                    output.parent.mkdir(exist_ok=True)
                    output.write_text(json.dumps(evidence, indent=2), encoding="utf-8")
                    raise
            print(json.dumps(result, indent=2))
        finally:
            inference.shutdown()
            collector.shutdown()
            inference.server_close()
            collector.server_close()


if __name__ == "__main__":
    main()
