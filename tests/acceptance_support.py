"""Process and event helpers shared by local and remote acceptance tests."""

from __future__ import annotations

import json
import os
import queue
import signal
import socket
import subprocess
import threading
import time
import urllib.error
import urllib.request
import uuid
from collections import defaultdict, deque
from pathlib import Path

from inference_fixture import require


def debug_binary(name):
    suffix = ".exe" if os.name == "nt" else ""
    return Path(__file__).resolve().parents[1] / "target" / "debug" / (name + suffix)


def require_context_usage(events, expected_limit):
    """Verify the real SDK's current-window event crosses the transport."""
    usage = [event["data"] for event in events if event["kind"] == "context_usage"]
    require(bool(usage), "SDK context usage event was not forwarded")
    latest = usage[-1]
    require(type(latest.get("currentTokens")) is int and latest["currentTokens"] > 0,
            "Context usage did not report the current token count")
    require(latest.get("tokenLimit") == expected_limit,
            f"Context usage lost the configured total window: {latest.get('tokenLimit')}")
    return latest


class SessionEvents:
    """One reader, with unmatched sessions retained for subsequent assertions."""

    def __init__(self):
        self.incoming = queue.Queue()
        self.pending = defaultdict(deque)

    def put(self, event):
        self.incoming.put(event)

    def next(self, session_id, deadline):
        if self.pending[session_id]:
            return self.pending[session_id].popleft()
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(
                    f"Session {session_id}: expected event did not arrive"
                )
            try:
                event = self.incoming.get(timeout=remaining)
            except queue.Empty:
                raise TimeoutError(
                    f"Session {session_id}: expected event did not arrive"
                ) from None
            if event["sessionId"] == session_id:
                return event
            self.pending[event["sessionId"]].append(event)


class Application:
    """Run the HTTP development host, not the remote stdio-only server."""

    def __init__(self, binary, directory, ephemeral_secrets=False, env=None):
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        self.base = f"http://127.0.0.1:{port}"
        self.log_path = directory / "server.log"
        self.log = self.log_path.open("a+")
        self.process = None
        self.response = None
        self.reader = None
        self.reader_error = None
        self.closed = False
        self.events = SessionEvents()
        self.ready = threading.Event()
        arguments = [
            str(binary),
            "--port",
            str(port),
            "--data-dir",
            str(directory / "data"),
        ]
        if ephemeral_secrets:
            arguments.append("--ephemeral-secrets")
        try:
            self.process = subprocess.Popen(
                arguments, stdout=self.log, stderr=self.log, env=env
            )
            self.wait_ready()
            self.reader = threading.Thread(target=self.read_events, daemon=True)
            self.reader.start()
            if not self.ready.wait(5):
                raise TimeoutError("Application event stream did not connect")
            if self.reader_error:
                raise RuntimeError(
                    "Application event stream failed"
                ) from self.reader_error
        except BaseException:
            self.close()
            raise

    def wait_ready(self):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError(
                    "Application exited during startup: " + self.log_tail()
                )
            try:
                urllib.request.urlopen(self.base + "/api/health", timeout=1).close()
                return
            except urllib.error.URLError:
                time.sleep(0.1)
        raise TimeoutError("Application server did not become ready")

    def read_events(self):
        try:
            self.response = urllib.request.urlopen(
                self.base + "/api/events", timeout=180
            )
            self.ready.set()
            for line in self.response:
                if line.startswith(b"data:"):
                    self.events.put(json.loads(line[5:]))
        except (OSError, ValueError) as error:
            self.reader_error = error
        finally:
            self.ready.set()

    def command(self, command, **payload):
        request = urllib.request.Request(
            self.base + "/api/dispatch",
            json.dumps({"command": command, "payload": payload}).encode(),
            {"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(request, timeout=180) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            raise RuntimeError(f"{command}: {error.read().decode()}") from error

    def wait_event(self, session_id, predicate, timeout=30):
        deadline = time.monotonic() + timeout
        while True:
            event = self.events.next(session_id, deadline)
            if predicate(event):
                return event
            if event["kind"] == "error":
                raise AssertionError(event["data"])

    def wait_idle(self, session_id):
        return self.wait_event(
            session_id,
            lambda event: (
                event["kind"] == "status" and event["data"]["status"] == "idle"
            ),
        )

    def wait_title(self, session_id, events=(), timeout=90):
        title_events = [
            event for event in events if event["kind"] in {"title", "title_error"}
        ]
        event = title_events[-1] if title_events else self.wait_event(
            session_id,
            lambda event: event["kind"] in {"title", "title_error"},
            timeout=timeout,
        )
        require(event["kind"] == "title", "Title generation failed")
        title = event["data"].get("title", "")
        require(bool(title.strip()) and len(title) <= 80, "Generated title is empty or too long")
        saved = next(
            session for session in self.command("snapshot")["sessions"]
            if session["id"] == session_id
        )
        require(saved["title"] == title, "Title event and persisted title disagree")
        return title

    def turn(self, session, prompt, approved=True):
        message_id = str(uuid.uuid4())
        self.command(
            "send_message",
            sessionId=session["id"],
            content=prompt,
            messageId=message_id,
        )
        events = []
        replied = False
        deadline = time.monotonic() + 90
        while True:
            event = self.events.next(session["id"], deadline)
            events.append(event)
            if event["kind"] == "permission":
                self.command(
                    "respond_permission",
                    sessionId=session["id"],
                    requestId=event["data"]["requestId"],
                    approved=approved,
                )
            if event["kind"] == "error":
                raise AssertionError(event["data"])
            if event["kind"] == "message" and event["data"].get("role") == "assistant":
                replied = True
            if (
                replied
                and event["kind"] == "status"
                and event["data"]["status"] == "idle"
            ):
                messages = self.command("messages", sessionId=session["id"])
                require(
                    any(
                        message["id"] == message_id
                        and message["role"] == "user"
                        and message["content"] == prompt
                        for message in messages
                    ),
                    "Confirmed user message lost the client's request identity",
                )
                return events

    def abort_streaming_turn(self, session, fixture):
        self.command(
            "send_message",
            sessionId=session["id"],
            content="fixture:slow Hold the stream for cancellation.",
            messageId=str(uuid.uuid4()),
        )
        self.wait_event(session["id"], lambda event: event["kind"] == "delta")
        self.command("abort_session", sessionId=session["id"])
        fixture.release_slow_response.set()
        self.wait_idle(session["id"])

    def abort_pending_permission(self, session, fixture):
        self.command(
            "send_message",
            sessionId=session["id"],
            content="fixture:cancel-permission Request the write and wait for cancellation.",
            messageId=str(uuid.uuid4()),
        )
        event = self.wait_event(
            session["id"], lambda event: event["kind"] == "permission"
        )
        self.command("abort_session", sessionId=session["id"])
        try:
            self.command(
                "respond_permission",
                sessionId=session["id"],
                requestId=event["data"]["requestId"],
                approved=True,
            )
        except RuntimeError:
            pass
        else:
            raise AssertionError("An approval remained usable after cancellation")
        require(
            not (fixture.workspace.parent / "cancelled.txt").exists(),
            "Cancelled permission executed a file write",
        )
        self.wait_idle(session["id"])

    def crash_runtime_during_turn(self, session, fixture):
        """Kill only this test server's SDK child and wait for the UI error event."""
        fixture.release_slow_response.clear()
        self.command(
            "send_message",
            sessionId=session["id"],
            content="fixture:slow Hold the stream for unexpected process termination.",
            messageId=str(uuid.uuid4()),
        )
        self.wait_event(session["id"], lambda event: event["kind"] == "delta")
        processes = subprocess.check_output(
            ["ps", "-Ao", "pid=,ppid=,comm="], text=True
        ).splitlines()
        children = [
            int(pid)
            for line in processes
            for pid, parent, command in [line.split(None, 2)]
            if int(parent) == self.process.pid
            and Path(command).name in {"copilot", "copilot-runtime"}
        ]
        require(len(children) == 1, "Could not identify this test's SDK child process")
        os.kill(children[0], signal.SIGKILL)
        fixture.release_slow_response.set()
        self.wait_event(session["id"], lambda event: event["kind"] == "error")

    def log_tail(self):
        if not self.log.closed:
            self.log.flush()
        return self.log_path.read_text()[-12000:]

    def close(self):
        if self.closed:
            return
        self.closed = True
        if self.response and self.response.fp:
            try:
                self.response.fp.raw._sock.shutdown(socket.SHUT_RDWR)
            except (AttributeError, OSError):
                pass
        if self.process and self.process.poll() is None:
            if os.name == "nt":
                self.process.terminate()
            else:
                self.process.send_signal(signal.SIGINT)
            try:
                self.process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        if self.reader:
            self.reader.join(timeout=2)
        if self.response:
            self.response.close()
        self.log.close()
