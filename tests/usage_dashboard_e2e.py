#!/usr/bin/env python3
"""Verify durable dashboard statistics with the real SDK and isolated fixtures.

No external provider or telemetry receiver is contacted. Build both service
binaries first, then run python3 tests/usage_dashboard_e2e.py.
"""

from __future__ import annotations

import argparse
import json
import tempfile
import threading
import time
import uuid
from datetime import datetime, timedelta, timezone
from http.server import ThreadingHTTPServer
from pathlib import Path

from acceptance_support import Application, debug_binary
from inference_fixture import FIXTURE_KEY, Fixture, handler_for, require
from remote_protocol import StdioServer, private_environment
from skill_acceptance import selection, write_project_skills


def day_ranges(count, end=None):
    """Use UTC calendar days in this fixture; UI tests cover local DST boundaries."""
    tomorrow = (end or datetime.now(timezone.utc)).replace(hour=0, minute=0, second=0, microsecond=0) + timedelta(days=1)
    first = tomorrow - timedelta(days=count)
    return [{"date": (first + timedelta(days=index)).date().isoformat(),
             "start": (first + timedelta(days=index)).isoformat(),
             "end": (first + timedelta(days=index + 1)).isoformat()}
            for index in range(count)]


def dashboard(app, days=7):
    return app.command("get_usage_dashboard", days=day_ranges(days))


def wait_usage(app, expected):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        report = dashboard(app)
        if report["totalTokens"] >= expected:
            return report
        time.sleep(.05)
    raise AssertionError(f"Usage ledger did not reach {expected} fixture tokens")


def token_totals(report):
    totals = {}
    for row in report["dailyTokens"]:
        key = (row["providerId"], row["modelId"])
        totals[key] = totals.get(key, 0) + row["totalTokens"]
        require(row["totalTokens"] == row["inputTokens"] + row["outputTokens"],
                "Dashboard double-counted or lost token categories")
    return totals


def reject_ranges(app):
    for days in ([], day_ranges(8), list(reversed(day_ranges(7)))):
        try:
            app.command("get_usage_dashboard", days=days)
        except RuntimeError:
            continue
        raise AssertionError("Dashboard accepted an invalid date window")


def check_local(binary, root, fixture, endpoint):
    root.mkdir()
    home = root / "home"
    home.mkdir()
    environment = private_environment(home)
    app = Application(binary, root, env=environment)
    try:
        empty = dashboard(app)
        require(empty["totalTokens"] == empty["skillInvocations"] == 0,
                "Fresh data fabricated historical usage")
        require(empty["dailyTokens"] == empty["dailySkills"] == empty["recentSessions"] == [],
                "Fresh dashboard contains non-existent sessions or events")
        require(bool(empty["trackingStartedAt"]), "Dashboard omitted collection start")
        reject_ranges(app)
        provider = app.command("save_provider", provider={
            "id": "", "name": "Dashboard fixture", "protocol": "openai-completions",
            "baseUrl": endpoint + "/v1", "models": [
                {"id": model, "name": model, "contextWindow": 128000, "maxOutputTokens": 4096}
                for model in ("gpt-4.1", "gpt-4.1-mini")],
        }, apiKey=FIXTURE_KEY)
        chat = app.command("create_session", projectId=None,
                           providerId=provider["id"], modelId="gpt-4.1")
        app.turn(chat, "fixture:chat Count a plain Chat turn without telemetry export.")
        first = wait_usage(app, 110)
        require(first["totalTokens"] == 110, "A single fixture completion was not counted once")
        chat = app.command("set_session_model", sessionId=chat["id"],
                           providerId=provider["id"], modelId="gpt-4.1-mini")
        app.turn(chat, "fixture:model-switch Count the changed model without rewriting older usage.")
        switched = wait_usage(app, 220)
        require(token_totals(switched) == {
            (provider["id"], "gpt-4.1"): 110,
            (provider["id"], "gpt-4.1-mini"): 110,
        }, "Model switching relabeled or duplicated prior consumption")
        other = app.command("save_provider", provider={
            "id": "", "name": "Second dashboard provider", "protocol": "openai-completions",
            "baseUrl": endpoint + "/v1", "models": [
                {"id": "gpt-4.1", "name": "Shared model ID", "contextWindow": 128000,
                 "maxOutputTokens": 4096}],
        }, apiKey=FIXTURE_KEY)
        chat = app.command("set_session_model", sessionId=chat["id"],
                           providerId=other["id"], modelId="gpt-4.1")
        app.turn(chat, "fixture:chat Count the same model ID under a different provider.")
        changed = wait_usage(app, 330)
        require(token_totals(changed) == {
            (provider["id"], "gpt-4.1"): 110,
            (provider["id"], "gpt-4.1-mini"): 110,
            (other["id"], "gpt-4.1"): 110,
        }, "Provider switching merged distinct provider/model series")
        project = app.command("create_project", name="Dashboard fixture",
                              path=str(fixture.workspace), connection={"kind": "local"})
        skills = write_project_skills(fixture.workspace, home)
        work = app.command("create_session", projectId=project["id"],
                           providerId=provider["id"], modelId="gpt-4.1")
        app.command("list_project_skills", projectId=project["id"])
        require(dashboard(app)["skillInvocations"] == 0,
                "Listing available skills counted as an invocation")
        before = len(fixture.conversation_requests())
        app.turn(work, "fixture:skill-auto Invoke the dashboard acceptance skill.")
        requests = len(fixture.conversation_requests()) - before
        expected = 330 + requests * 110
        report = wait_usage(app, expected)
        require(report["totalTokens"] == expected, "Work inference usage differs from fixture responses")
        require(report["skillInvocations"] == 1 and sum(
            row["count"] for row in report["dailySkills"] if row["skillName"] == skills["auto"]["name"]
        ) == 1, "Real skill invocation was not recorded exactly once")
        for worker in ("first", "second"):
            (fixture.workspace / f"fleet-{worker}.txt").write_text(f"{worker} worker evidence\n")
        app.turn(work, "fixture:fleet Inspect both files independently, then summarize.", fleet=True)
        requests = len(fixture.conversation_requests()) - before
        expected = 330 + requests * 110
        report = wait_usage(app, expected)
        require(report["totalTokens"] == expected, "Fleet parent or subagent inference was omitted or counted twice")
        history = app.command("messages", sessionId=work["id"])
        require(sum(message["role"] == "agent" for message in history) == 2,
                "Fleet fixture did not execute two real subagents")
        require(report["skillInvocations"] == 1, "Restored skill context counted as a new invocation")
        app.turn(work, f"fixture:skill-explicit Follow ${skills['manual']['name']} selected in the composer.",
                 selected_skills=[selection(skills["manual"])])
        requests = len(fixture.conversation_requests()) - before
        expected = 330 + requests * 110
        report = wait_usage(app, expected)
        require(report["totalTokens"] == expected and report["skillInvocations"] == 2,
                "Explicitly invoked skill did not add one native invocation")
        require(sum(row["count"] for row in report["dailySkills"]
                    if row["skillName"] == skills["manual"]["name"]) == 1,
                "Explicit invocation was not attributed to its skill")
        sessions = {row["sessionId"]: row["totalTokens"] for row in report["recentSessions"]}
        require(sessions == {chat["id"]: 330, work["id"]: requests * 110},
                "Recent sessions lost Chat/Work totals")
        for count in (14, 30):
            require(dashboard(app, count)["totalTokens"] == expected,
                    "Changing date range changed same-day usage")
        old = app.command("get_usage_dashboard", days=day_ranges(7, datetime.now(timezone.utc) - timedelta(days=60)))
        require(old["totalTokens"] == old["skillInvocations"] == 0,
                "Historical range included current usage")
        require(not fixture.exports, "Local dashboard unexpectedly enabled external telemetry")
    finally:
        app.close()
    restarted = Application(binary, root, env=environment)
    try:
        persisted = dashboard(restarted)
        for field in ("trackingStartedAt", "dailyTokens", "dailySkills", "totalTokens", "skillInvocations"):
            require(persisted[field] == report[field], f"Restart changed durable {field}")
        restarted.turn(chat, "fixture:chat Continue after restart without replaying old usage.")
        resumed = wait_usage(restarted, expected + 110)
        require(resumed["totalTokens"] == expected + 110,
                "Resume replay duplicated historical usage")
        restarted.command("delete_session", sessionId=work["id"])
        after_delete = dashboard(restarted)
        require(after_delete["totalTokens"] == resumed["totalTokens"] and
                after_delete["dailySkills"] == resumed["dailySkills"],
                "Conversation deletion unexpectedly erased aggregate consumption")
        require(all(row["sessionId"] != work["id"] for row in after_delete["recentSessions"]),
                "Deleted conversation remained in recent sessions")
    finally:
        restarted.close()
    return expected + 110


def check_remote(binary, root, endpoint, workspace):
    root.mkdir()
    session_id = str(uuid.uuid4())
    server = StdioServer(binary, root)
    try:
        require(server.command("server_info")["protocolVersion"] == 4,
                "Remote statistics require protocol version 4")
        options = {
            "sessionId": session_id, "mode": {"kind": "chat"}, "permissionMode": "ask",
            "provider": {"protocol": "openai-completions", "baseUrl": endpoint + "/v1", "apiKey": FIXTURE_KEY},
            "model": {"id": "gpt-4.1", "contextWindow": 128000, "maxOutputTokens": 4096},
            "systemPrompt": "Return the requested synthetic fixture response.",
        }
        telemetry = {"enabled": False, "endpoint": "", "protocol": "http/json", "captureContent": False}
        server.command("start_agent", options=options, resume=False, telemetry=telemetry)
        events = server.turn(session_id, "fixture:chat Verify usage crosses the remote stdio boundary.")
        usage = [event["data"] for event in events if event["kind"] == "usage"]
        require(len(usage) == 1, "Remote usage event missing or duplicated")
        require(usage[0]["inputTokens"] == 100 and usage[0]["outputTokens"] == 10,
                "Remote usage counts differ from inference fixture")
        require(bool(usage[0]["eventId"]) and bool(usage[0]["timestamp"]),
                "Remote usage lost deduplication or time identity")
        server.command("disconnect_agent", sessionId=session_id)
        work_id = str(uuid.uuid4())
        options.update(sessionId=work_id, mode={"kind": "work", "directory": str(workspace)})
        server.command("start_agent", options=options, resume=False, telemetry=telemetry)
        events = server.turn(work_id, "fixture:skill-auto Verify remote skill invocation reporting.")
        skills = [event["data"] for event in events if event["kind"] == "skill_usage"]
        require(len(skills) == 1 and skills[0]["skillName"] == "tandem-fixture-auto",
                "Remote skill invocation missing or duplicated")
        require(bool(skills[0]["eventId"]) and bool(skills[0]["timestamp"]),
                "Remote skill lost deduplication or time identity")
    finally:
        server.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=debug_binary("tandem-dev-server"))
    parser.add_argument("--remote-server", type=Path, default=debug_binary("tandem-server"))
    arguments = parser.parse_args()
    require(arguments.server.is_file() and arguments.remote_server.is_file(), "Build both service binaries first")
    with tempfile.TemporaryDirectory(prefix="tandem-usage-e2e-") as temporary:
        root = Path(temporary)
        workspace = root / "project"
        workspace.mkdir()
        fixture = Fixture(workspace)
        endpoint = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(fixture))
        threading.Thread(target=endpoint.serve_forever, daemon=True).start()
        try:
            url = f"http://127.0.0.1:{endpoint.server_port}"
            tokens = check_local(arguments.server.resolve(), root / "local", fixture, url)
            check_remote(arguments.remote_server.resolve(), root / "remote", url, workspace)
            print(json.dumps({"status": "passed", "localTokens": tokens,
                              "coverage": ["Chat", "Work", "model switch", "skill invocation", "restart", "remote stdio"]}))
        finally:
            endpoint.shutdown()
            endpoint.server_close()


if __name__ == "__main__":
    main()
