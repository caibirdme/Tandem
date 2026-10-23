"""Exercise permission policy changes through the real local or SSH runtime."""

import json
import uuid

from acceptance_support import activity_tools
from inference_fixture import require


def auto_turn(app, session, prompt):
    # A policy regression must fail; the harness must never approve it silently.
    events = app.turn(session, prompt, approved=False)
    require(
        not any(event["kind"] == "permission" for event in events),
        "Automatically authorized session still requested an interactive approval",
    )
    failed_tools = [
        {"name": tool["name"], "error": tool.get("error")}
        for tool in activity_tools(events)
        if tool["status"] == "error"
    ]
    require(
        not failed_tools,
        "Automatically authorized tool execution failed: " + json.dumps(failed_tools),
    )
    return events


def check_chat_permissions(app, fixture, session):
    before = len(fixture.conversation_requests())
    events = auto_turn(
        app, session, "fixture:chat-fetch Read the web page using web_fetch."
    )
    require(
        bool(activity_tools(events)),
        "Chat did not execute its web tool",
    )
    requests = fixture.conversation_requests()[before:]
    require(len(requests) >= 2, "Web tool result did not reach inference")
    require(
        "Contents of https://example.com:" in str(requests[-1][1]),
        "Chat web tool did not read the expected public example page",
    )
    require(
        not app.command("snapshot")["activeTurns"].get(session["id"], {}).get("permissions"),
        "Chat retained a pending approval",
    )


def check_permission_modes(app, project, provider, model, workspace):
    session = app.command(
        "create_session",
        projectId=project["id"],
        providerId=provider["id"],
        modelId=model,
    )
    require(session["permissionMode"] == "allow-all", "New Work session did not default to full access")
    for action in ("create", "read", "edit", "shell"):
        auto_turn(app, session, f"fixture:work-mode-{action} Exercise the real tool.")
    require(
        (workspace / "permission-mode.txt").read_text() == "automatic edited\n",
        "Fully authorized file tools did not create and edit the expected file",
    )
    require(
        (workspace / "mode-shell.txt").read_text().strip() == "shell approved",
        "Fully authorized shell tool did not execute",
    )
    before = app.command("messages", sessionId=session["id"])
    changed = app.command(
        "set_permission_mode", sessionId=session["id"], permissionMode="ask"
    )
    require(changed["permissionMode"] == "ask", "Could not return to interactive mode")
    require(
        app.command("messages", sessionId=session["id"]) == before,
        "Changing permission mode changed conversation history",
    )
    app.command(
        "send_message",
        sessionId=session["id"],
        content="fixture:work-mode-ask Request a write that will be denied.",
        messageId=str(uuid.uuid4()),
    )
    permission = app.wait_event(session["id"], lambda event: event["kind"] == "permission")
    try:
        app.command(
            "set_permission_mode", sessionId=session["id"], permissionMode="allow-all"
        )
    except RuntimeError:
        pass
    else:
        raise AssertionError("Permission policy changed while approval was pending")
    pending_session = next(
        item for item in app.command("snapshot")["sessions"] if item["id"] == session["id"]
    )
    require(
        pending_session["permissionMode"] == "ask",
        "Rejected policy change still altered the session",
    )
    app.command(
        "respond_permission",
        sessionId=session["id"],
        requestId=permission["data"]["requestId"],
        approved=False,
    )
    app.wait_idle(session["id"])
    require(not (workspace / "mode-denied.txt").exists(), "Denied write still executed")
    session = app.command(
        "set_permission_mode", sessionId=session["id"], permissionMode="allow-all"
    )
    auto_turn(app, session, "fixture:work-mode-ask Retry under explicit full authorization.")
    require(
        (workspace / "mode-denied.txt").read_text() == "mode changed\n",
        "Resumed work did not apply its changed permission mode",
    )
    return session
