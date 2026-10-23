#!/usr/bin/env python3
"""Exercise steering through the real bundled SDK with local inference only.

The fixture holds either an inference response or a real shell tool. This tests
acceptance separately from consumption, including the turn-end fallback. No
provider credentials, paid inference, or public network services are needed.
"""

from __future__ import annotations

import argparse
import json
import shlex
import tempfile
import threading
import time
import uuid
from http.server import ThreadingHTTPServer
from pathlib import Path

from acceptance_support import Application, debug_binary
from attachment_acceptance import attachment_inputs
from usage_dashboard_e2e import wait_usage
from inference_fixture import FIXTURE_KEY, Fixture, handler_for, is_title_request, require
from remote_protocol import StdioServer, private_environment


class SteeringFixture(Fixture):
    def __init__(self, workspace):
        super().__init__(workspace)
        self.cases = {}

    def case(self, tool=False):
        token = uuid.uuid4().hex
        case = {"token": token, "tool": tool, "started": threading.Event(),
                "release": threading.Event(), "issued": False, "timeline": [],
                "start_file": self.workspace / (token + ".started"),
                "release_file": self.workspace / (token + ".release"),
                "finished_file": self.workspace / (token + ".finished")}
        self.cases[token] = case
        return case

    def response(self, body):
        if is_title_request(body):
            return super().response(body)
        messages = body.get("messages", [])
        text = json.dumps(messages, ensure_ascii=False)
        latest_user = next((item for item in reversed(messages) if item.get("role") == "user"), {})
        if "idle-steer:" in json.dumps(latest_user):
            return "TANDEM_IDLE_STEERED"
        for token, case in self.cases.items():
            if token not in text:
                continue
            case["timeline"].append({"event": "inference", "at": time.monotonic(),
                                     "steering": "steer:" + token in text,
                                     "toolResults": [item.get("content", "") for item in messages[-3:]
                                                     if item.get("role") == "tool"]})
            if "steer:" + token in text:
                return "TANDEM_STEERED_" + token
            if not case["issued"]:
                case["issued"] = True
                case["started"].set()
                if not case["tool"]:
                    require(case["release"].wait(30), "Held inference was not released")
                    return "TANDEM_INITIAL_" + token
                names = {tool.get("function", tool).get("name") for tool in body.get("tools", [])}
                if "powershell" in names and "bash" not in names:
                    quote = lambda path: "'" + str(path).replace("'", "''") + "'"
                    command = (f"Set-Content -LiteralPath {quote(case['start_file'])} -Value started; "
                               "$deadline = (Get-Date).AddSeconds(30); "
                               f"while (!(Test-Path -LiteralPath {quote(case['release_file'])})) {{ "
                               "if ((Get-Date) -gt $deadline) { throw 'Steering gate timed out' }; "
                               "Start-Sleep -Milliseconds 50 }; "
                               f"Set-Content -LiteralPath {quote(case['finished_file'])} -Value finished")
                    tool = "powershell"
                else:
                    quote = lambda path: shlex.quote(str(path))
                    command = (f"printf started > {quote(case['start_file'])}; "
                               f"i=0; while [ ! -f {quote(case['release_file'])} ]; do "
                               "i=$((i+1)); [ $i -lt 600 ] || exit 1; sleep 0.05; done; "
                               f"printf finished > {quote(case['finished_file'])}")
                    tool = "bash"
                return {"name": tool, "arguments": {"command": command,
                        "description": "Wait for the steering acceptance gate"}}
            return "TANDEM_INITIAL_" + token
        return super().response(body)

    def unblock(self, case):
        case["release"].set()
        case["release_file"].touch()

    def unblock_all(self):
        for case in self.cases.values():
            self.unblock(case)


def wait_started(case):
    require(case["started"].wait(30), "The real SDK did not call fixture inference")
    if case["tool"]:
        deadline = time.monotonic() + 30
        while not case["start_file"].exists() and time.monotonic() < deadline:
            time.sleep(.025)
        require(case["start_file"].exists(), "The real shell tool never started")


def reject(call, message):
    try:
        call()
    except RuntimeError:
        return
    raise AssertionError(message)


def receive(app, session_id, marker):
    events = []
    seen = False
    deadline = time.monotonic() + 60
    while True:
        event = app.events.next(session_id, deadline)
        events.append(event)
        require(event["kind"] != "error", f"Steering caused a session error: {event['data']}")
        if event["kind"] == "message" and event["data"].get("role") == "assistant":
            seen |= marker in event["data"].get("content", "")
        if seen and event["kind"] == "status" and event["data"]["status"] == "idle":
            return events


def check_local(app, fixture, provider, project, tool, skill=None):
    session = app.command("create_session", projectId=project["id"] if tool else None,
                          providerId=provider["id"], modelId="gpt-4.1")
    case = fixture.case(tool)
    session_id, token = session["id"], case["token"]
    original_id = str(uuid.uuid4())
    app.command("send_message", sessionId=session_id, content="start:" + token, messageId=original_id)
    try:
        wait_started(case)
        reject(lambda: app.command("send_message", sessionId=session_id, content="Must not overlap",
                                   messageId=str(uuid.uuid4())), "Ordinary send bypassed its busy guard")
        reject(lambda: app.command("steer_message", sessionId=session_id, content="Invalid selection",
                                   messageId=str(uuid.uuid4()),
                                   selectedSkills=[{"name": "missing", "path": str(fixture.workspace / "missing.md")}]),
               "Invalid steering skills were accepted")
        ids = []
        attachments, attachment_marker = attachment_inputs()
        for index in range(2):
            message_id = str(uuid.uuid4())
            ids.append(message_id)
            app.command("steer_message", sessionId=session_id,
                        content=f"steer:{token} instruction-{index}", messageId=message_id,
                        selectedSkills=[skill] if skill is not None and index == 0 else [],
                        attachments=attachments if index == 0 else [])
        reject(lambda: app.command("steer_message", sessionId=session_id,
                                   content=f"steer:{token} duplicate", messageId=ids[0]),
               "Steering accepted a duplicate persisted message ID")
        saved = next(item for item in app.command("snapshot")["sessions"] if item["id"] == session_id)
        require(saved["status"] == "running", "Steering reset the active session status")
        require(not case["finished_file"].exists(), "Steering preempted the shell gate")
    finally:
        fixture.unblock(case)
    events = receive(app, session_id, "TANDEM_STEERED_" + token)
    if tool:
        first_reply = next(index for index, event in enumerate(events)
                           if event["kind"] == "message" and event["data"].get("role") == "assistant"
                           and "TANDEM_STEERED_" + token in event["data"].get("content", ""))
        require(not any(event["kind"] == "status" and event["data"]["status"] == "idle"
                        for event in events[:first_reply]),
                "Work steering was queued until after the active turn instead of injected")
        require(case["finished_file"].exists(), "The in-flight tool did not complete normally")
    requests = [body for _path, body in fixture.conversation_requests()
                if "steer:" + token in json.dumps(body)]
    require(bool(requests), "Accepted steering never reached an inference request")
    require(all("start:" + token in json.dumps(body) for body in requests),
            "Steering lost the existing SDK conversation context")
    require(any(attachment_marker in json.dumps(body) for body in requests),
            "Steering dropped the text attachment before inference")
    require(any(attachments[0]["data"] in json.dumps(body) for body in requests),
            "Steering dropped the image attachment before inference")
    require(all(any(f"instruction-{index}" in json.dumps(body) for body in requests) for index in range(2)),
            "One of the consecutive steering messages was lost")
    messages = app.command("messages", sessionId=session_id)
    require([item["id"] for item in messages if item["role"] == "user"] == [original_id, *ids],
            "Steering history duplicated or reordered user messages")
    attached = next(item for item in messages if item["id"] == ids[0])
    require(len(attached["attachments"]) == len(attachments), "Steering attachment metadata was lost")
    for metadata, original in zip(attached["attachments"], attachments):
        restored = app.command("get_attachment", sessionId=session_id, attachmentId=metadata["id"])
        require(restored == original, "Steering attachment content did not persist")
    if skill is not None:
        require(any("TANDEM_STEERING_SKILL" in json.dumps(body) for body in requests),
                "The selected skill body did not reach the steered inference")
    # Calling explicit steer after idle uses the ordinary turn path, not a lost
    # immediate message or a second live SDK session.
    idle_id = str(uuid.uuid4())
    app.command("steer_message", sessionId=session_id, content="idle-steer: Continue in the same session.",
                messageId=idle_id)
    receive(app, session_id, "TANDEM_IDLE_STEERED")
    require(any(item["id"] == idle_id for item in app.command("messages", sessionId=session_id)),
            "The idle fallback lost its user message")


def check_abort(app, fixture, provider, project):
    """Stop drops steering queued behind a genuinely unfinished inference.

    A shell is not a suitable barrier here: the CLI may background it upon
    steering, consuming the prompt before the subsequent Stop request arrives.
    The held HTTP response makes the unconsumed-message precondition explicit.
    """
    session = app.command("create_session", projectId=project["id"],
                          providerId=provider["id"], modelId="gpt-4.1")
    case = fixture.case(False)
    session_id, token = session["id"], case["token"]
    offset = len(fixture.conversation_requests())
    try:
        app.command("send_message", sessionId=session_id, content="start:" + token,
                    messageId=str(uuid.uuid4()))
        wait_started(case)
        app.command("steer_message", sessionId=session_id, content="steer:" + token,
                    messageId=str(uuid.uuid4()))
        case["timeline"].append({"event": "steer_ack", "at": time.monotonic()})
        require(len(fixture.conversation_requests()) == offset + 1,
                "The held inference did not keep steering pending before Stop")
        app.command("abort_session", sessionId=session_id)
        case["timeline"].append({"event": "abort_ack", "at": time.monotonic()})
        saved = next(item for item in app.command("snapshot")["sessions"] if item["id"] == session_id)
        require(saved["status"] == "idle", "Stop did not settle a session with pending steering")
        fixture.unblock(case)
        app.command("send_message", sessionId=session_id, content="idle-steer: Continue after stopping.",
                    messageId=str(uuid.uuid4()))
        receive(app, session_id, "TANDEM_IDLE_STEERED")
        require(len(fixture.conversation_requests()) == offset + 2,
                "Canceled steering caused an extra inference instead of just the explicit next turn")
    except BaseException:
        print(json.dumps({"abortTimeline": case["timeline"]}, indent=2))
        raise
    finally:
        fixture.unblock(case)


def check_remote(binary, root, endpoint, fixture):
    root.mkdir()
    server = StdioServer(binary, root)
    case = fixture.case(True)
    token, session_id = case["token"], str(uuid.uuid4())
    try:
        require(server.command("server_info")["protocolVersion"] >= 5,
                "The remote server does not advertise the steering protocol")
        server.command("start_agent", options={
            "sessionId": session_id, "mode": {"kind": "work", "directory": str(fixture.workspace)},
            "permissionMode": "allow-all",
            "provider": {"protocol": "openai-completions", "baseUrl": endpoint + "/v1", "apiKey": FIXTURE_KEY},
            "model": {"id": "gpt-4.1", "contextWindow": 128000, "maxOutputTokens": 4096},
        }, resume=False, telemetry={"enabled": False, "endpoint": "", "protocol": "http/json", "captureContent": False})
        server.command("send_agent", sessionId=session_id, content="start:" + token)
        wait_started(case)
        prepared = str(uuid.uuid4())
        server.command("prepare_agent_steering", sessionId=session_id, preparedId=prepared,
                       content="steer:" + token, selectedSkills=[], attachments=[])
        server.command("send_prepared_agent_message", sessionId=session_id, preparedId=prepared)
        require(not case["finished_file"].exists(), "Remote steering preempted the running tool")
        fixture.unblock(case)
        deadline, seen = time.monotonic() + 60, False
        while True:
            event = server.events.get(timeout=max(.1, deadline - time.monotonic()))
            require(event["sessionId"] == session_id, "Remote steering crossed session boundaries")
            require(event["kind"] != "error", "Remote steering caused an error")
            seen |= "TANDEM_STEERED_" + token in json.dumps(event)
            if seen and event["kind"] == "status" and event["data"]["state"] == "idle":
                break
            require(time.monotonic() < deadline, "Remote steering was not consumed")
        require(case["finished_file"].exists(), "Remote steering interrupted the existing tool")
    finally:
        fixture.unblock(case)
        server.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=debug_binary("tandem-dev-server"))
    parser.add_argument("--remote-server", type=Path, default=debug_binary("tandem-server"))
    args = parser.parse_args()
    require(args.server.is_file() and args.remote_server.is_file(), "Build both server binaries first")
    with tempfile.TemporaryDirectory(prefix="tandem-steering-e2e-") as temporary:
        root = Path(temporary)
        workspace, home = root / "project", root / "home"
        workspace.mkdir()
        home.mkdir()
        path = workspace / ".agents/skills/steering-check/SKILL.md"
        path.parent.mkdir(parents=True)
        path.write_text("---\nname: steering-check\ndescription: Verify explicit steering selection\n"
                        "disable-model-invocation: true\n---\nTANDEM_STEERING_SKILL\n", encoding="utf-8")
        fixture = SteeringFixture(workspace)
        endpoint = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(fixture))
        threading.Thread(target=endpoint.serve_forever, daemon=True).start()
        url = f"http://127.0.0.1:{endpoint.server_port}"
        app = Application(args.server.resolve(), root, env=private_environment(home))
        try:
            provider = app.command("save_provider", provider={
                "id": "", "name": "Steering fixture", "protocol": "openai-completions", "baseUrl": url + "/v1",
                "models": [{"id": "gpt-4.1", "name": "Fixture", "contextWindow": 128000,
                            "maxOutputTokens": 4096}],
            }, apiKey=FIXTURE_KEY)
            project = app.command("create_project", name="Steering acceptance", path=str(workspace),
                                  connection={"kind": "local"})
            catalog = app.command("list_project_skills", projectId=project["id"])
            selected = next(item for item in catalog if item["name"] == "steering-check")
            skill = {key: selected[key] for key in ("name", "path")}
            check_local(app, fixture, provider, project, False)
            check_local(app, fixture, provider, project, True, skill)
            check_abort(app, fixture, provider, project)
            report = wait_usage(app, 110)
            require(all(item["totalTokens"] > 0 for item in report["recentSessions"]),
                    "Steering lost session token attribution")
            require(len(report["recentSessions"]) == 3, "Steering created an extra application session")
            check_remote(args.remote_server.resolve(), root / "remote", url, fixture)
            print(json.dumps({"result": "passed", "coverage": [
                "real SDK with held inference and real shell tool", "Chat and Work steering",
                "two consecutive immediate messages", "invalid skill and duplicate rejection",
                "explicit skill expansion while busy", "idle fallback", "durable message identity",
                "attachment persistence and provider payload", "session usage attribution",
                "remote two-phase steering protocol", "no tool interruption",
                "stop clears pending steering behind held inference and permits the next ordinary turn",
            ]}, indent=2))
        except BaseException:
            print(app.log_tail())
            raise
        finally:
            fixture.unblock_all()
            app.close()
            endpoint.shutdown()
            endpoint.server_close()


if __name__ == "__main__":
    main()
