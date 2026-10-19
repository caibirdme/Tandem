#!/usr/bin/env python3
"""Opt-in acceptance against the four real inference models requested for Tandem.

Build tandem-dev-server first, then run this script and enter the provider key at
its hidden prompt. A non-terminal stdin supplies one key line instead. Never put
the key in command arguments or environment variables. --models selects retries;
--output receives a redacted JSON report after every chat/work case.

Only this script's temporary workspace is approved. The local observation proxy
forwards each request body unchanged to the real HTTPS gateway. It records only
model, path, token limits, streaming mode, status, and elapsed time. No requests
are sent by --self-test. No operating-system credential store is used.
"""

from __future__ import annotations

import argparse
import getpass
import hmac
import http.client
import json
import os
import secrets
import shlex
import sys
import tempfile
import threading
import time
import unittest
import urllib.parse
import uuid
from contextlib import suppress
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from acceptance_support import Application, debug_binary
from credential_checks import require_provider_key, scan_for_key
from inference_fixture import is_title_request

GATEWAY_HOST = "camp-dev.woa.com"
GATEWAY_PREFIX = "/pingress"
CONTEXT_WINDOW = 1_000_000
ANTHROPIC_OUTPUT_LIMIT = 128_000
MAX_REQUEST_BYTES = 16 * 1024 * 1024
HOP_HEADERS = {
    "host",
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "expect",
}


@dataclass(frozen=True)
class Model:
    id: str
    protocol: str


MODELS = (
    Model("claude-haiku-5.5", "anthropic"),
    Model("glm-5.3-flashx", "anthropic"),
    Model("gpt-6.1-sol", "openai-responses"),
    Model("deepseek-v4.1-flash", "openai-responses"),
)


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def redact(value, key):
    """Apply once more at the output boundary, including unexpected error text."""
    if isinstance(value, str):
        return value.replace(key, "[redacted]") if key else value
    if isinstance(value, dict):
        return {redact(name, key): redact(item, key) for name, item in value.items()}
    if isinstance(value, list):
        return [redact(item, key) for item in value]
    return value


class Observations:
    def __init__(self, models):
        self.models = {model.id: model for model in models}
        self.requests = []
        self.lock = threading.Lock()

    def record(self, path, body):
        payload = json.loads(body)
        require(isinstance(payload, dict), "Inference body must be an object")
        model = self.models.get(payload.get("model"))
        require(model is not None, "Proxy received an unexpected model")
        expected = "/v1/messages" if model.protocol == "anthropic" else "/v1/responses"
        require(
            urllib.parse.urlsplit(path).path == GATEWAY_PREFIX + expected,
            "Proxy received an unexpected endpoint",
        )
        record = {
            "model": model.id,
            "protocol": model.protocol,
            "path": GATEWAY_PREFIX + expected,
            "max_tokens": payload.get("max_tokens"),
            "max_output_tokens": payload.get("max_output_tokens"),
            "stream": payload.get("stream") is True,
            "httpStatus": None,
            "purpose": "title" if is_title_request(payload) else "conversation",
            "toolCount": len(payload.get("tools", [])),
        }
        # Token fields are numbers in the protocol, never echo arbitrary strings.
        for field in ("max_tokens", "max_output_tokens"):
            if record[field] is not None and type(record[field]) is not int:
                record[field] = "invalid-type"
        with self.lock:
            self.requests.append(record)
        return record

    def finish(self, record, status, elapsed, failure=None):
        with self.lock:
            record.update(httpStatus=status, elapsedSeconds=round(elapsed, 3))
            if failure:
                record["transportError"] = failure

    def snapshot(self):
        with self.lock:
            return [dict(request) for request in self.requests]


def proxy_handler(observations, key, timeout):
    class GatewayProxy(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, _format, *_arguments):
            pass

        def do_POST(self):
            authorization = self.headers.get("authorization", "")
            api_key = self.headers.get("x-api-key", "")
            authenticated = hmac.compare_digest(
                authorization, "Bearer " + key
            ) or hmac.compare_digest(api_key, key)
            if not authenticated:
                self.send_error(401, "Local proxy authentication required")
                return
            try:
                length = int(self.headers.get("content-length", "-1"))
                require(0 <= length <= MAX_REQUEST_BYTES, "Invalid request size")
                body = self.rfile.read(length)
                require(len(body) == length, "Truncated request body")
                record = observations.record(self.path, body)
            except (AssertionError, TypeError, ValueError):
                self.send_error(400, "Invalid inference request")
                return
            connection = http.client.HTTPSConnection(GATEWAY_HOST, timeout=timeout)
            started = time.monotonic()
            status = None
            headers_sent = False
            try:
                headers = {
                    name: value
                    for name, value in self.headers.items()
                    if name.lower() not in HOP_HEADERS
                }
                # These exact input bytes, including token settings, go upstream.
                connection.request("POST", self.path, body=body, headers=headers)
                response = connection.getresponse()
                status = response.status
                observations.finish(record, status, time.monotonic() - started)
                self.send_response(status)
                for name, value in response.getheaders():
                    if name.lower() not in HOP_HEADERS:
                        self.send_header(name, value)
                self.send_header("Connection", "close")
                self.end_headers()
                headers_sent = True
                self.close_connection = True
                while block := response.read1(64 * 1024):
                    self.wfile.write(block)
                    self.wfile.flush()
            except (OSError, http.client.HTTPException) as error:
                observations.finish(
                    record, status, time.monotonic() - started, type(error).__name__
                )
                if not headers_sent:
                    self.send_error(502, "Upstream transport failed")
            finally:
                connection.close()
                observations.finish(record, status, time.monotonic() - started)

    return GatewayProxy


class WorkPolicy:
    """An allowlist of the exact test resources, never model-authored shell text."""

    def __init__(self, workspace, command):
        self.workspace = workspace.resolve()
        self.command = command
        self.approved = 0
        self.approved_commands = 0
        self.denied = 0

    def permits(self, request):
        details = request.get("details")
        if not isinstance(details, dict):
            return False
        kind = request.get("kind")
        if kind == "shell":
            command = details.get("fullCommandText", details.get("command"))
            return isinstance(command, str) and command.strip() == self.command
        if kind not in {"read", "write"}:
            return False
        paths = [details[field] for field in ("fileName", "path") if field in details]
        if not paths or any(not isinstance(path, str) or not path for path in paths):
            return False
        for value in paths:
            path = Path(value)
            if not path.is_absolute():
                path = self.workspace / path
            path = path.resolve()
            if not path.is_relative_to(self.workspace):
                return False
            if kind == "write" and path != self.workspace / "result.txt":
                return False
        return True

    def decide(self, request):
        approved = self.permits(request)
        if approved:
            self.approved += 1
            if request.get("kind") == "shell":
                self.approved_commands += 1
        else:
            self.denied += 1
        return approved


def run_turn(app, session, prompt, policy, timeout):
    message_id = str(uuid.uuid4())
    try:
        app.command(
            "send_message",
            sessionId=session["id"],
            content=prompt,
            messageId=message_id,
        )
        return wait_turn(app, session, prompt, message_id, policy, timeout)
    except Exception:
        # Keep the original failure; cancellation failures must not hide it.
        with suppress(Exception):
            app.command("abort_session", sessionId=session["id"])
        raise


def wait_turn(app, session, prompt, message_id, policy, timeout):
    deadline = time.monotonic() + timeout
    events = []
    while True:
        event = app.events.next(session["id"], deadline)
        events.append(event)
        if event["kind"] == "permission":
            approved = policy.decide(event["data"]) if policy else False
            app.command(
                "respond_permission",
                sessionId=session["id"],
                requestId=event["data"]["requestId"],
                approved=approved,
            )
        if event["kind"] == "error":
            raise RuntimeError(event["data"].get("message", "Agent reported an error"))
        if event["kind"] == "status" and event["data"].get("status") == "idle":
            messages = app.command("messages", sessionId=session["id"])
            require(
                any(
                    message["id"] == message_id and message["content"] == prompt
                    for message in messages
                ),
                "User message was not persisted with its request identity",
            )
            assistant = [
                event["data"]["content"]
                for event in events
                if event["kind"] == "message"
                and event["data"].get("role") == "assistant"
            ]
            require(assistant, "No completed assistant response")
            require(
                all(
                    any(
                        message["role"] == "assistant" and message["content"] == content
                        for message in messages
                    )
                    for content in assistant
                ),
                "Assistant response was not persisted",
            )
            require(
                any(
                    event["kind"] == "delta" and event["data"].get("content")
                    for event in events
                ),
                "No streaming assistant text",
            )
            return events


def tool_evidence(events):
    names = {}
    arguments = {}
    successful = []
    for event in events:
        if event["kind"] != "tool":
            continue
        data = event["data"]
        call_id = data.get("toolCallId")
        if data.get("name") not in {None, "tool"}:
            names[call_id] = data["name"]
        if data.get("status") == "running":
            with suppress(TypeError, ValueError):
                parsed = json.loads(data.get("content", ""))
                if isinstance(parsed, dict):
                    arguments[call_id] = parsed
        if data.get("status") == "completed":
            successful.append(
                (
                    names.get(call_id, data.get("name", "tool")),
                    arguments.get(call_id, {}),
                    data.get("content", ""),
                )
            )
    return successful


def create_session(app, provider, model, project_id=None):
    return app.command(
        "create_session",
        projectId=project_id,
        providerId=provider["id"],
        modelId=model.id,
    )


def check_chat(app, provider, model, timeout):
    nonce = "TANDEM_CHAT_" + secrets.token_hex(12)
    session = create_session(app, provider, model)
    events = run_turn(
        app,
        session,
        f"Reply with exactly {nonce}. Do not use any tools.",
        None,
        timeout,
    )
    require(
        any(
            event["kind"] == "message"
            and event["data"].get("role") == "assistant"
            and event["data"].get("content", "").strip() == nonce
            for event in events
        ),
        "Chat did not return the unpredictable nonce",
    )
    title = app.wait_title(session["id"], events, timeout=timeout)
    return {"streaming": True, "persisted": True, "nonceConfirmed": True, "title": title}


def check_work(app, provider, model, directory, timeout):
    directory.mkdir()
    nonce = secrets.token_hex(12)
    numbers = [secrets.randbelow(90) + 10 for _ in range(3)]
    seed = {"nonce": nonce, "numbers": numbers}
    (directory / "seed.json").write_text(json.dumps(seed), encoding="utf-8")
    draft = f"nonce={nonce}\ntotal={sum(numbers)}\nstatus=draft\n"
    final = draft.replace("status=draft", "status=verified")
    marker = "TANDEM_VERIFIED_" + nonce
    verifier = directory / "verify.py"
    source = (
        "from pathlib import Path\n"
        "result = Path(__file__).with_name('result.txt').read_text(encoding='utf-8')\n"
        f"assert result == {final!r}, 'Unexpected result file contents'\n"
        f"print({marker!r})\n"
    )
    verifier.write_text(source, encoding="utf-8")
    command_parts = [sys.executable, "-I", str(verifier)]
    command = (
        "& " + " ".join("'" + part.replace("'", "''") + "'" for part in command_parts)
        if os.name == "nt"
        else shlex.join(command_parts)
    )
    policy = WorkPolicy(directory, command)
    project = app.command(
        "create_project",
        name="Live acceptance " + model.id,
        path=str(directory),
        connection={"kind": "local"},
    )
    session = create_session(app, provider, model, project["id"])
    first = run_turn(
        app,
        session,
        "In this temporary project, use the file viewing tool to read seed.json. "
        "Then use the file creation tool to create result.txt with exactly three lines and a final newline: "
        "nonce=<the nonce from seed.json>\ntotal=<sum of its numbers>\nstatus=draft\n"
        "Do not use shell commands or access any other directory. Do not read or modify verify.py. "
        "After the file exists, reply briefly describing what you verified.",
        policy,
        timeout,
    )
    require(
        (directory / "result.txt").read_text(encoding="utf-8") == draft,
        "Work did not create the correct file from the seed data",
    )
    first_tools = tool_evidence(first)
    title = app.wait_title(session["id"], first, timeout=timeout)
    require(
        any(
            name in {"view", "read_file"}
            and "seed.json" in str(arguments.get("path", ""))
            for name, arguments, _ in first_tools
        ),
        "No successful seed-reading tool evidence",
    )
    require(
        any(
            name in {"create", "apply_patch", "write_file"}
            for name, _, _ in first_tools
        ),
        "No successful file-creation tool evidence",
    )
    second = run_turn(
        app,
        session,
        "Use the file editing tool to replace only status=draft with status=verified in result.txt. "
        "Do not rewrite the whole file, and do not modify verify.py or seed.json. "
        f"Then use the shell tool to execute exactly this verification command, without wrappers or additions:\n{command}\n"
        "No other shell commands, network access, or paths outside this project are allowed. "
        "Report the verification command's output after it finishes.",
        policy,
        timeout,
    )
    require(
        (directory / "result.txt").read_text(encoding="utf-8") == final,
        "Work did not edit the real file correctly",
    )
    require(
        verifier.read_text(encoding="utf-8") == source,
        "Work changed the trusted verifier",
    )
    require(
        json.loads((directory / "seed.json").read_text(encoding="utf-8")) == seed,
        "Work changed the seed data",
    )
    second_tools = tool_evidence(second)
    require(
        any(
            name in {"edit", "apply_patch", "str_replace_editor"}
            for name, _, _ in second_tools
        ),
        "No successful editing tool evidence",
    )
    require(
        any(
            name in {"bash", "powershell", "read_bash", "read_powershell"}
            and marker in output
            for name, _, output in second_tools
        ),
        "No successful command output containing the verifier nonce",
    )
    require(
        any(
            name in {"bash", "powershell"}
            and arguments.get("command", "").strip() == command
            for name, arguments, _ in second_tools
        ),
        "No tool invocation of the exact verification command",
    )
    require(
        policy.approved_commands > 0,
        "Verification command did not pass the explicit approval boundary",
    )
    return {
        "streaming": True,
        "persisted": True,
        "readCreateEditVerified": True,
        "commandOutputVerified": True,
        "approvals": policy.approved,
        "denials": policy.denied,
        "title": title,
    }


def write_report(report, output, key):
    safe = redact(report, key)
    if output:
        output.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            dir=output.parent,
            prefix=".tandem-live-report-",
            delete=False,
        ) as stream:
            temporary = Path(stream.name)
            json.dump(safe, stream, ensure_ascii=False, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        temporary.replace(output)
    return safe


def execute(arguments, key):
    selected = [model for model in MODELS if model.id in arguments.models]
    observations = Observations(selected)
    report = {
        "status": "running",
        "contextWindow": CONTEXT_WINDOW,
        "cases": [],
        "requests": [],
    }
    exit_code = 0
    with tempfile.TemporaryDirectory(prefix="tandem-live-provider-") as temporary:
        root = Path(temporary)
        app_directory = root / "application"
        app_directory.mkdir()
        environment = os.environ.copy()
        for name in (
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "COPILOT_GITHUB_TOKEN",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
        ):
            environment.pop(name, None)
        proxy = ThreadingHTTPServer(
            ("127.0.0.1", 0), proxy_handler(observations, key, arguments.timeout)
        )
        proxy.daemon_threads = True
        threading.Thread(target=proxy.serve_forever, daemon=True).start()
        app = None
        try:
            app = Application(
                arguments.server.resolve(), app_directory, ephemeral_secrets=True, env=environment
            )
            for model in selected:
                base = f"http://127.0.0.1:{proxy.server_port}{GATEWAY_PREFIX}"
                if model.protocol == "openai-responses":
                    base += "/v1"
                provider = app.command(
                    "save_provider",
                    provider={
                        "id": "",
                        "name": "Live " + model.id,
                        "protocol": model.protocol,
                        "baseUrl": base,
                        "models": [
                            {
                                "id": model.id,
                                "name": model.id,
                                "contextWindow": CONTEXT_WINDOW,
                                "maxOutputTokens": ANTHROPIC_OUTPUT_LIMIT
                                if model.protocol == "anthropic"
                                else None,
                            }
                        ],
                    },
                    apiKey=key,
                )
                require_provider_key(app, provider["id"], key)
                for mode in ("chat", "work"):
                    started = time.monotonic()
                    request_start = len(observations.snapshot())
                    case = {"model": model.id, "protocol": model.protocol, "mode": mode}
                    try:
                        details = (
                            check_chat(app, provider, model, arguments.timeout)
                            if mode == "chat"
                            else check_work(
                                app, provider, model, root / model.id, arguments.timeout
                            )
                        )
                        requests = observations.snapshot()[request_start:]
                        require(requests, "The case made no inference requests")
                        title_requests = [request for request in requests if request["purpose"] == "title"]
                        require(title_requests, "The case did not generate a title through the real model")
                        require(all(request["toolCount"] == 0 for request in title_requests),
                                "Title generation advertised tools")
                        if model.protocol == "anthropic":
                            require(
                                all(
                                    request["max_tokens"] == ANTHROPIC_OUTPUT_LIMIT
                                    for request in requests
                                ),
                                "Actual Anthropic max_tokens was not 128000 on every request",
                            )
                        case.update(status="passed", **details)
                    except (
                        AssertionError,
                        OSError,
                        RuntimeError,
                        TimeoutError,
                        ValueError,
                    ) as error:
                        case.update(status="failed", error=redact(str(error), key))
                        exit_code = 1
                    case["elapsedSeconds"] = round(time.monotonic() - started, 3)
                    case["inferenceRequests"] = (
                        len(observations.snapshot()) - request_start
                    )
                    report["cases"].append(case)
                    report["requests"] = observations.snapshot()
                    write_report(report, arguments.output, key)
                    print(f"{model.id} {mode}: {case['status']}", flush=True)
        except (
            AssertionError,
            OSError,
            RuntimeError,
            TimeoutError,
            ValueError,
        ) as error:
            report["error"] = redact(str(error), key)
            exit_code = 1
        finally:
            if app:
                app.close()
            proxy.shutdown()
            proxy.server_close()
            leaks = scan_for_key(app_directory, key)
            report["credentialScan"] = {
                "passed": not leaks,
                "leakedRelativePaths": leaks,
            }
            if leaks:
                exit_code = 1
            report["requests"] = observations.snapshot()
            anthropic = [
                request
                for request in report["requests"]
                if request["protocol"] == "anthropic"
            ]
            limits_valid = all(
                request["max_tokens"] == ANTHROPIC_OUTPUT_LIMIT for request in anthropic
            )
            report["anthropicMaxTokens"] = {
                "passed": limits_valid,
                "requestsChecked": len(anthropic),
                "expected": ANTHROPIC_OUTPUT_LIMIT,
            }
            if not limits_valid:
                exit_code = 1
            report["status"] = "passed" if exit_code == 0 else "failed"
            safe = write_report(report, arguments.output, key)
            print(json.dumps(safe, ensure_ascii=False, indent=2), flush=True)
    return exit_code


def self_test():
    suite = unittest.defaultTestLoader.loadTestsFromName("test_live_provider_e2e")
    return 0 if unittest.TextTestRunner().run(suite).wasSuccessful() else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=debug_binary("tandem-dev-server"))
    parser.add_argument(
        "--models",
        nargs="+",
        choices=[model.id for model in MODELS],
        default=[model.id for model in MODELS],
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="Write a redacted JSON progress report after each case",
    )
    parser.add_argument("--timeout", type=int, choices=[120, 180], default=180)
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="Test the harness's policy without credentials or network requests",
    )
    arguments = parser.parse_args()
    if arguments.self_test:
        return self_test()
    if not arguments.server.is_file():
        parser.error("Build tandem-dev-server first or supply --server")
    key = (
        getpass.getpass("Provider API key: ")
        if sys.stdin.isatty()
        else sys.stdin.readline().rstrip("\r\n")
    )
    if not key or any(character in key for character in ("\r", "\n", "\0")):
        parser.error("A non-empty API key is required on stdin")
    try:
        return execute(arguments, key)
    except (AssertionError, OSError, RuntimeError, TimeoutError, ValueError) as error:
        print(
            json.dumps({"status": "failed", "error": redact(str(error), key)}),
            file=sys.stderr,
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
