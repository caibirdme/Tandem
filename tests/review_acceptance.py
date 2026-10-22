#!/usr/bin/env python3
"""Verify immutable reviews with real Git, the real SDK, and test inference.

Run after building tandem-dev-server and tandem-server. The reusable Work flow
also runs over a real isolated SSH connection in remote_e2e.py.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import tempfile
import threading
import uuid
from http.server import ThreadingHTTPServer
from pathlib import Path

from acceptance_support import Application, debug_binary
from inference_fixture import FIXTURE_KEY, Fixture, handler_for, require
from remote_protocol import StdioServer, private_environment


def git(directory, *arguments):
    return subprocess.run(
        ["git", "-c", "user.name=Acceptance", "-c", "user.email=acceptance@example.invalid",
         "-c", "core.autocrlf=false", "-C", str(directory), *arguments],
        check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    ).stdout


def initialize_project(directory):
    directory.mkdir(parents=True)
    git(directory, "init", "--initial-branch=main")
    for name in ("existing.txt", "cancelled.txt"):
        (directory / name).write_text("original\n", encoding="utf-8")
    git(directory, "add", "--", "existing.txt", "cancelled.txt")
    git(directory, "commit", "-m", "Initial review fixture")
    (directory / "existing.txt").write_text("user change before the session\n", encoding="utf-8")
    (directory / "cancelled.txt").write_text("staged change\n", encoding="utf-8")
    git(directory, "add", "--", "cancelled.txt")
    (directory / "cancelled.txt").write_text("original\n", encoding="utf-8")
    (directory / "preexisting.txt").write_text("user untracked file\n", encoding="utf-8")


def overview(app, session, selection):
    result = app.command("get_review_summary", sessionId=session["id"], selection=selection)
    require(result["summary"] is not None, f"Review is unavailable: {result['unavailable']}")
    require(result["unavailable"] is None, "A usable review also reported an unavailable error")
    return result


def files_for(app, session, summary):
    files, cursor = [], 0
    while cursor is not None:
        page = app.command("list_review_files", sessionId=session["id"],
                           reviewId=summary["reviewId"], revision=summary["revision"], cursor=cursor)
        files.extend(page["files"])
        next_cursor = page["nextCursor"]
        require(next_cursor is None or next_cursor > cursor, "Review cursor did not progress")
        cursor = next_cursor
    require(len(files) == summary["filesTotal"], "Review count and file pages disagree")
    return {file["path"]: file for file in files}


def detail(app, session, summary, file):
    return app.command("get_review_file", sessionId=session["id"],
                       reviewId=summary["reviewId"], revision=summary["revision"], fileId=file["id"])


def require_rejected(app, command, **payload):
    try:
        app.command(command, **payload)
    except RuntimeError:
        return
    raise AssertionError(f"{command} accepted a review outside its session/revision")


def latest_user_id(app, session):
    return [message["id"] for message in app.command("messages", sessionId=session["id"])
            if message["role"] == "user"][-1]


def check_review_flow(app, fixture, provider, directory, connection):
    """Run identical session lifecycle assertions locally and over SSH."""
    initialize_project(directory)
    original_workspace = fixture.workspace
    fixture.workspace = directory
    project = app.command("create_project", name="Review acceptance", path=str(directory), connection=connection)
    session = app.command("create_session", projectId=project["id"],
                          providerId=provider["id"], modelId="gpt-4.1")
    try:
        status_before = git(directory, "status", "--porcelain=v2", "-z")
        index_before = (directory / ".git" / "index").read_bytes()
        initial = overview(app, session, {"kind": "workspace", "comparison": "net"})["summary"]
        initial_files = files_for(app, session, initial)
        require(set(initial_files) == {"existing.txt", "cancelled.txt", "preexisting.txt"},
                f"Workspace inventory lost dirty, staged cancellation, or untracked files: {initial_files}")
        for comparison in ("staged", "unstaged"):
            summary = overview(app, session, {"kind": "workspace", "comparison": comparison})["summary"]
            require("cancelled.txt" in files_for(app, session, summary),
                    f"{comparison} omitted a staged change cancelled in the working tree")
        require(git(directory, "status", "--porcelain=v2", "-z") == status_before,
                "Reading reviews changed the project state")
        require((directory / ".git" / "index").read_bytes() == index_before,
                "Reading reviews rewrote the user's index")
        unavailable = app.command("get_review_summary", sessionId=session["id"], selection={"kind": "session"})
        require(unavailable["summary"] is None and unavailable["unavailable"]["code"] == "baseline_unavailable",
                "A session without a first turn invented a baseline")

        first_events = app.turn(session, "fixture:work-create Create the review acceptance file.")
        first_id = latest_user_id(app, session)
        require(any(event["kind"] == "review_updated" for event in first_events),
                "The session did not publish review updates")
        first = overview(app, session, {"kind": "turn", "messageId": first_id})["summary"]
        first_files = files_for(app, session, first)
        require(set(first_files) == {"verified.txt"},
                "A turn review attributed preexisting user changes to the turn")
        frozen = detail(app, session, first, first_files["verified.txt"])
        require("first version" in (frozen["patch"] or ""), "Created file content is missing from the review")
        require(first["complete"], "A completed captured turn was marked incomplete")

        app.turn(session, "fixture:work-edit Edit the review acceptance file.")
        second_id = latest_user_id(app, session)
        second = overview(app, session, {"kind": "turn", "messageId": second_id})["summary"]
        second_files = files_for(app, session, second)
        second_diff = detail(app, session, second, second_files["verified.txt"])
        require("first version" in (second_diff["patch"] or "")
                and "verified by SDK" in (second_diff["patch"] or ""),
                "The second turn did not compare against its own start")
        require(detail(app, session, first, first_files["verified.txt"]) == frozen,
                "A later turn changed an immutable historical review")

        app.turn(session, "fixture:work-shell Write the shell review marker.")
        shell_id = latest_user_id(app, session)
        shell = overview(app, session, {"kind": "turn", "messageId": shell_id})["summary"]
        require(set(files_for(app, session, shell)) == {"shell-approved.txt"},
                "A shell-created file was absent from the turn review")

        app.turn(session, "fixture:review-commit Change verified.txt and commit that file.")
        require((directory / "verified.txt").read_text().strip() == "committed by SDK",
                "The real SDK did not execute the edit-and-commit tool")
        commit_id = latest_user_id(app, session)
        committed = overview(app, session, {"kind": "turn", "messageId": commit_id})["summary"]
        committed_files = files_for(app, session, committed)
        require(set(committed_files) == {"verified.txt"}, "Committing a file erased its turn review")
        committed_diff = detail(app, session, committed, committed_files["verified.txt"])
        require("verified by SDK" in (committed_diff["patch"] or "")
                and "committed by SDK" in (committed_diff["patch"] or ""),
                "A commit changed the turn's before/after content")
        current = overview(app, session, {"kind": "workspace", "comparison": "net"})["summary"]
        require("verified.txt" not in files_for(app, session, current),
                "The committed file remained in the workspace inventory")
        accumulated = overview(app, session, {"kind": "session"})
        require(set(files_for(app, session, accumulated["summary"])) == {"verified.txt", "shell-approved.txt"},
                "The session comparison lost committed changes or included preexisting edits")
        require(len(accumulated["turns"]) == 4, "Review turn navigation lost a submitted turn")

        fixture.release_slow_response.clear()
        app.abort_streaming_turn(session, fixture)
        interrupted_id = latest_user_id(app, session)
        interrupted = overview(app, session, {"kind": "turn", "messageId": interrupted_id})["summary"]
        require(not interrupted["complete"], "An interrupted turn was reported as a complete review")
        require(detail(app, session, first, first_files["verified.txt"]) == frozen,
                "Stopping another turn changed historical review content")

        require_rejected(app, "get_review_file", sessionId=session["id"], reviewId=first["reviewId"],
                         revision="wrong-revision", fileId=first_files["verified.txt"]["id"])
        other = app.command("create_session", projectId=project["id"],
                            providerId=provider["id"], modelId="gpt-4.1")
        require_rejected(app, "list_review_files", sessionId=other["id"],
                         reviewId=first["reviewId"], revision=first["revision"])
        app.command("delete_session", sessionId=other["id"])
        return session, first_id, first, first_files["verified.txt"], frozen
    finally:
        fixture.workspace = original_workspace


def check_protocol(remote_server, root):
    workspace = root / "protocol-project"
    initialize_project(workspace)
    home = root / "protocol-home"
    home.mkdir()
    server = StdioServer(remote_server, home)
    workspace_id = str(uuid.uuid4())
    try:
        require(not server.command("server_info")["runtimeReady"], "The remote runtime started eagerly")
        rejected = server.request("get_workspace_review", workspaceId=workspace_id, comparison="net")
        require("error" in rejected, "An unbound remote workspace could be read")
        server.command("open_review_workspace", workspaceId=workspace_id, directory=str(workspace))
        before = server.command("capture_review_snapshot", workspaceId=workspace_id, previous=None)
        (workspace / "preexisting.txt").write_text("remote snapshot change\n", encoding="utf-8")
        after = server.command("capture_review_snapshot", workspaceId=workspace_id, previous=before["id"])
        summary = server.command("compare_review_snapshots", workspaceId=workspace_id,
                                 before=before["id"], after=after["id"], selection={"kind": "session"})
        page = server.command("list_review_files", workspaceId=workspace_id,
                              reviewId=summary["reviewId"], revision=summary["revision"])
        require([file["path"] for file in page["files"]] == ["preexisting.txt"],
                "Remote snapshot comparison lost the raw file change")
        diff = server.command("get_review_file", workspaceId=workspace_id,
                              reviewId=summary["reviewId"], revision=summary["revision"],
                              fileId=page["files"][0]["id"])
        require("remote snapshot change" in diff["patch"], "Remote detail lost source content")
        rejected = server.request("open_review_workspace", workspaceId=workspace_id, directory=str(root))
        require("error" in rejected, "A remote binding could be redirected to another directory")
        require(not server.command("server_info")["runtimeReady"], "Review inspection initialized the SDK")
    finally:
        server.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=debug_binary("tandem-dev-server"))
    parser.add_argument("--remote-server", type=Path, default=debug_binary("tandem-server"))
    arguments = parser.parse_args()
    require(arguments.server.is_file() and arguments.remote_server.is_file(), "Build both server binaries first")
    with tempfile.TemporaryDirectory(prefix="tandem-review-e2e-") as temporary:
        root = Path(temporary)
        home = root / "home"
        home.mkdir()
        environment = private_environment(home)
        fixture = Fixture(root)
        endpoint = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(fixture))
        threading.Thread(target=endpoint.serve_forever, daemon=True).start()
        app = Application(arguments.server.resolve(), root, env=environment)
        try:
            provider = app.command("save_provider", provider={
                "id": "", "name": "Review fixture", "protocol": "openai-completions",
                "baseUrl": f"http://127.0.0.1:{endpoint.server_port}/v1",
                "models": [{"id": "gpt-4.1", "name": "Fixture", "contextWindow": 128000,
                            "maxOutputTokens": 4096}],
            }, apiKey=FIXTURE_KEY)
            plain = root / "plain-directory"
            plain.mkdir()
            project = app.command("create_project", name="No Git", path=str(plain), connection={"kind": "local"})
            session = app.command("create_session", projectId=project["id"],
                                  providerId=provider["id"], modelId="gpt-4.1")
            unavailable = app.command("get_review_summary", sessionId=session["id"],
                                      selection={"kind": "workspace", "comparison": "net"})
            require(unavailable["summary"] is None and unavailable["unavailable"]["code"] == "not_repository",
                    "A non-Git directory was presented as an empty review")
            require(not app.command("snapshot")["runtime"]["available"],
                    "Opening a review started the SDK runtime")
            app.turn(session, "fixture:chat Work remains usable without Git.")
            chat = app.command("create_session", projectId=None, providerId=provider["id"], modelId="gpt-4.1")
            require_rejected(app, "get_review_summary", sessionId=chat["id"], selection={"kind": "session"})
            session, message_id, summary, file, frozen = check_review_flow(
                app, fixture, provider, root / "project with spaces", {"kind": "local"},
            )
            app.close()
            app = Application(arguments.server.resolve(), root, env=environment)
            restored = overview(app, session, {"kind": "turn", "messageId": message_id})["summary"]
            require(restored == summary, "Restart changed the saved turn summary")
            require(detail(app, session, summary, file) == frozen, "Restart lost a saved historical diff")
            require(not app.command("snapshot")["runtime"]["available"],
                    "Reading a historical review after restart started the SDK")
            check_protocol(arguments.remote_server.resolve(), root)
            print(json.dumps({"result": "passed", "coverage": [
                "real Git and SDK with test inference", "workspace net/staged/unstaged", "dirty first baseline",
                "per-turn and session snapshots", "shell and committed edits", "immutable revisions",
                "interrupted turn", "restart persistence", "session ownership", "non-Git Work",
                "remote review RPC without SDK",
            ]}, indent=2))
        finally:
            app.close()
            endpoint.shutdown()
            endpoint.server_close()


if __name__ == "__main__":
    main()
