"""Verify model changes against the real SDK's history and inference requests."""

import json
import sqlite3
from contextlib import closing

from inference_fixture import require
from permission_acceptance import auto_turn


ALTERNATE_MODEL = "gpt-4.1-mini"


def stored_session(app, session_id):
    return next(
        item for item in app.command("snapshot")["sessions"]
        if item["id"] == session_id
    )


def require_saved_model(directory, session):
    database = directory / "data" / "relay.sqlite3"
    with closing(sqlite3.connect(database.as_uri() + "?mode=ro", uri=True)) as connection:
        row = connection.execute(
            "SELECT provider_id, data FROM sessions WHERE id=?", (session["id"],)
        ).fetchone()
    require(row is not None, "Model change lost the session's database row")
    saved = json.loads(row[1])
    require(
        row[0] == saved["providerId"] == session["providerId"]
        and saved["modelId"] == session["modelId"]
        and saved["permissionMode"] == session["permissionMode"],
        "Model selection, provider index, or work authorization was not persisted",
    )


def require_wire_history(body, transcript):
    messages = body.get("messages", body.get("input", []))
    require(isinstance(messages, list), "Resumed request omitted structured history")
    position = 0
    for message in transcript:
        if message["role"] not in {"user", "assistant"}:
            continue
        match = next(
            (
                index for index in range(position, len(messages))
                if messages[index].get("role") == message["role"]
                and message["content"] in wire_text(messages[index].get("content", ""))
            ),
            None,
        )
        require(match is not None, "New model did not receive prior conversation history in order")
        position = match + 1


def wire_text(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "".join(
            part.get("text", "") for part in content
            if isinstance(part, dict) and isinstance(part.get("text", ""), str)
        )
    return ""


def switch_model(app, directory, session, provider, model):
    before = stored_session(app, session["id"])
    transcript = app.command("messages", sessionId=session["id"])
    changed = app.command(
        "set_session_model", sessionId=session["id"],
        providerId=provider["id"], modelId=model,
    )
    require(
        changed["providerId"] == provider["id"] and changed["modelId"] == model,
        "Model change returned a different provider or model",
    )
    for field in ("id", "title", "titleState", "projectId", "permissionMode", "createdAt"):
        require(changed[field] == before[field], f"Model change altered session {field}")
    require(
        app.command("messages", sessionId=session["id"]) == transcript,
        "Model change rewrote persisted conversation history",
    )
    saved = stored_session(app, changed["id"])
    require(
        saved["providerId"] == provider["id"] and saved["modelId"] == model,
        "Model change was not reflected in the application snapshot",
    )
    require_saved_model(directory, changed)
    return changed


def check_wire_turn(app, fixture, session, prompt, expected_path):
    before = app.command("messages", sessionId=session["id"])
    offset = len(fixture.conversation_requests())
    events = app.turn(session, prompt)
    requests = [
        (path, body) for path, body in fixture.conversation_requests()[offset:]
        if prompt in json.dumps(body, ensure_ascii=False)
    ]
    require(bool(requests), "Model-change turn never reached the inference fixture")
    for path, body in requests:
        require(path.split("?", 1)[0] == expected_path, "Model change used the previous provider's wire protocol")
        require(body["model"] == session["modelId"], "Inference still used the previous model")
        require_wire_history(body, before)
    after = app.command("messages", sessionId=session["id"])
    require(after[:len(before)] == before, "Model-change turn lost or rewrote the original transcript")
    require(
        after[-1]["role"] == "assistant"
        and after[-1]["content"] == "RELAY_MODEL_SWITCH_OK " + session["modelId"],
        "New model response was not persisted in the original conversation",
    )
    return events


def reject_busy_switch(app, directory, session, provider, model):
    before = stored_session(app, session["id"])
    messages = app.command("messages", sessionId=session["id"])
    try:
        app.command(
            "set_session_model", sessionId=session["id"],
            providerId=provider["id"], modelId=model,
        )
    except RuntimeError:
        pass
    else:
        raise AssertionError("Model selection changed during generation or pending approval")
    saved = stored_session(app, session["id"])
    for field in ("providerId", "modelId", "permissionMode", "status"):
        require(saved[field] == before[field], f"Rejected model change still altered {field}")
    require(
        app.command("messages", sessionId=session["id"]) == messages,
        "Rejected model change modified the active conversation",
    )
    require_saved_model(directory, saved)


def check_model_switching(app, fixture, providers, directory, automatic):
    provider, original_model = providers["openai-completions"]
    chat = app.command(
        "create_session", projectId=None, providerId=provider["id"], modelId=original_model,
    )
    events = check_wire_turn(
        app, fixture, chat,
        "fixture:model-switch Keep MODEL_SWITCH_ORIGINAL_HISTORY for every later model.",
        "/v1/chat/completions",
    )
    app.wait_title(chat["id"], events)
    for protocol, model, path in (
        ("openai-completions", ALTERNATE_MODEL, "/v1/chat/completions"),
        ("openai-responses", providers["openai-responses"][1], "/v1/responses"),
        ("anthropic", providers["anthropic"][1], "/v1/messages"),
        ("openai-completions", ALTERNATE_MODEL, "/v1/chat/completions"),
    ):
        chat = switch_model(app, directory, chat, providers[protocol][0], model)
        check_wire_turn(
            app, fixture, chat,
            f"fixture:model-switch Continue MODEL_SWITCH_{protocol}_{model}.", path,
        )

    fixture.release_slow_response.clear()
    app.command(
        "send_message", sessionId=chat["id"],
        content="fixture:slow Hold this original-model response while switching is rejected.",
    )
    app.wait_event(chat["id"], lambda event: event["kind"] == "delta")
    try:
        reject_busy_switch(app, directory, chat, provider, original_model)
    finally:
        fixture.release_slow_response.set()
    app.wait_event(chat["id"], lambda event: event["kind"] == "message" and event["data"].get("role") == "assistant")
    app.wait_idle(chat["id"])
    require(
        app.command("messages", sessionId=chat["id"])[-1]["content"] == "RELAY_SLOW_OK",
        "Rejected model change interrupted the in-flight response",
    )

    automatic = switch_model(app, directory, automatic, provider, ALTERNATE_MODEL)
    auto_turn(app, automatic, "fixture:work-mode-read Read under the authorization retained after switching models.")
    require(
        fixture.conversation_requests()[-1][1]["model"] == ALTERNATE_MODEL,
        "Authorized work resumed with the previous model",
    )

    asking = app.command(
        "create_session", projectId=automatic["projectId"],
        providerId=provider["id"], modelId=original_model,
    )
    seed = app.turn(asking, "fixture:chat Seed the interactive work conversation.")
    app.wait_title(asking["id"], seed)
    app.command(
        "send_message", sessionId=asking["id"],
        content="fixture:model-switch-permission Request the write and retain the pending approval.",
    )
    permission = app.wait_event(asking["id"], lambda event: event["kind"] == "permission")
    reject_busy_switch(app, directory, asking, provider, ALTERNATE_MODEL)
    app.command(
        "respond_permission", sessionId=asking["id"],
        requestId=permission["data"]["requestId"], approved=True,
    )
    app.wait_event(asking["id"], lambda event: event["kind"] == "message" and event["data"].get("role") == "assistant")
    app.wait_idle(asking["id"])
    require(
        (fixture.workspace / "model-switch-approved.txt").read_text() == "model switch approval retained\n",
        "Rejected model change invalidated the original pending approval",
    )
    require(
        fixture.conversation_requests()[-1][1]["model"] == original_model,
        "Rejected model change altered the original approval's inference model",
    )
    require_saved_model(directory, stored_session(app, asking["id"]))
    return stored_session(app, chat["id"]), automatic


def check_model_switch_after_restart(app, fixture, directory, chat):
    restored = stored_session(app, chat["id"])
    require(
        restored["providerId"] == chat["providerId"] and restored["modelId"] == chat["modelId"],
        "Application restart lost the switched provider or model",
    )
    require_saved_model(directory, restored)
    check_wire_turn(
        app, fixture, restored,
        "fixture:model-switch Continue MODEL_SWITCH_AFTER_APPLICATION_RESTART with the entire original history.",
        "/v1/chat/completions",
    )
