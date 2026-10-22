"""Verify reasoning settings and exposed thinking through the production SDK."""

import copy
import json
import sqlite3
import uuid
from contextlib import closing

from inference_fixture import (
    FIXTURE_KEY,
    THINKING_CHUNKS,
    THINKING_SIGNATURE,
    THINKING_TEXT,
    require,
)
from model_switch_acceptance import require_wire_history, stored_session, switch_model


def wire_effort(path, body):
    endpoint = path.split("?", 1)[0]
    if endpoint.endswith("/chat/completions"):
        return body.get("reasoning_effort")
    if endpoint.endswith("/responses"):
        return body.get("reasoning", {}).get("effort")
    if endpoint.endswith("/v1/messages"):
        return body.get("output_config", {}).get("effort")
    raise AssertionError("Reasoning acceptance received an unsupported wire protocol")


def require_wire_effort(path, body, expected):
    """Check the provider request, rather than only the application's setting."""
    actual = wire_effort(path, body)
    require(actual == expected,
            f"Inference reasoning effort differs from the session selection: {actual!r} != {expected!r}")


def require_saved_effort(directory, session):
    database = directory / "data" / "tandem.sqlite3"
    with closing(sqlite3.connect(database.as_uri() + "?mode=ro", uri=True)) as connection:
        row = connection.execute("SELECT data FROM sessions WHERE id=?", (session["id"],)).fetchone()
    require(row is not None, "Reasoning change removed the stored session")
    require(json.loads(row[0])["reasoningEffort"] == session["reasoningEffort"],
            "Session reasoning override was not persisted")


def set_effort(app, directory, session, effort):
    before = stored_session(app, session["id"])
    messages = app.command("messages", sessionId=session["id"])
    changed = app.command("set_session_reasoning_effort", sessionId=session["id"], reasoningEffort=effort)
    require(changed["reasoningEffort"] == effort, "Reasoning change returned the wrong override")
    for field in ("id", "title", "titleState", "projectId", "providerId", "modelId",
                  "permissionMode", "createdAt", "status", "contextUsage"):
        require(changed[field] == before[field], f"Reasoning change altered session {field}")
    require(app.command("messages", sessionId=session["id"]) == messages,
            "Reasoning change rewrote the conversation")
    require(stored_session(app, session["id"])["reasoningEffort"] == effort,
            "Snapshot did not reflect the reasoning override")
    require_saved_effort(directory, changed)
    return changed


def check_reasoning_turn(app, fixture, session, prompt, expected):
    transcript = app.command("messages", sessionId=session["id"])
    offset = len(fixture.conversation_requests())
    events = app.turn(session, prompt)
    requests = [(path, body) for path, body in fixture.conversation_requests()[offset:]
                if prompt in json.dumps(body, ensure_ascii=False)]
    require(bool(requests), "Reasoning test never reached provider inference")
    for path, body in requests:
        require(body["model"] == session["modelId"], "Reasoning change selected a different model")
        require_wire_effort(path, body, expected)
        require_wire_history(body, transcript)
    require(app.command("messages", sessionId=session["id"])[:len(transcript)] == transcript,
            "Reasoning change lost or rewrote the original conversation")
    return events


def check_active_reasoning_change(app, fixture, directory, session, before, after):
    offset = len(fixture.conversation_requests())
    prompt = "fixture:slow Keep this response running during a reasoning change."
    fixture.release_slow_response.clear()
    app.command("send_message", sessionId=session["id"], content=prompt, messageId=str(uuid.uuid4()))
    app.wait_event(session["id"], lambda event: event["kind"] == "delta")
    try:
        session = set_effort(app, directory, session, after)
        active = [(path, body) for path, body in fixture.conversation_requests()[offset:]
                  if prompt in json.dumps(body)]
        require(len(active) == 1, "Reasoning change replayed the active user message")
        require_wire_effort(*active[0], before)
    finally:
        fixture.release_slow_response.set()
    app.wait_event(session["id"], lambda event: event["kind"] == "message"
                   and event["data"].get("role") == "assistant")
    app.wait_idle(session["id"])
    require(app.command("messages", sessionId=session["id"])[-1]["content"] == "TANDEM_SLOW_OK",
            "Reasoning change interrupted the active response")
    check_reasoning_turn(app, fixture, session,
                         "fixture:chat Apply the reasoning change to the next user turn.", after)
    return session


def reject_invalid_efforts(app, directory, session, provider, project):
    before = stored_session(app, session["id"])
    messages = app.command("messages", sessionId=session["id"])
    for effort in ("", "HIGH", " low ", "unsupported", 1, True, [], {}):
        for command, payload in (
            ("set_session_reasoning_effort", {"sessionId": session["id"]}),
            ("create_session", {"projectId": project["id"], "providerId": provider["id"],
                                "modelId": provider["models"][0]["id"]}),
        ):
            sessions = len(app.command("snapshot")["sessions"])
            try:
                app.command(command, **payload, reasoningEffort=effort)
            except RuntimeError:
                pass
            else:
                raise AssertionError(f"{command} accepted malformed reasoning effort {effort!r}")
            require(len(app.command("snapshot")["sessions"]) == sessions,
                    "Invalid reasoning effort created a session")
    for payload in ({"sessionId": session["id"]},
                    {"sessionId": "missing-session", "reasoningEffort": "high"}):
        try:
            app.command("set_session_reasoning_effort", **payload)
        except RuntimeError:
            pass
        else:
            raise AssertionError("Incomplete reasoning update or missing session was accepted")
    require(stored_session(app, session["id"]) == before,
            "Rejected reasoning effort modified the session")
    require(app.command("messages", sessionId=session["id"]) == messages,
            "Rejected reasoning effort modified the conversation")
    require_saved_effort(directory, before)


def check_pending_reasoning_change(app, fixture, directory, work):
    work = app.command("set_permission_mode", sessionId=work["id"], permissionMode="ask")
    prompt = "fixture:reasoning-permission Keep this pending approval during a reasoning change."
    offset = len(fixture.conversation_requests())
    app.command("send_message", sessionId=work["id"], content=prompt, messageId=str(uuid.uuid4()))
    permission = app.wait_event(work["id"], lambda event: event["kind"] == "permission")
    work = set_effort(app, directory, work, "high")
    app.command("respond_permission", sessionId=work["id"],
                requestId=permission["data"]["requestId"], approved=True)
    app.wait_event(work["id"], lambda event: event["kind"] == "message"
                   and event["data"].get("role") == "assistant")
    app.wait_idle(work["id"])
    require((fixture.workspace / "reasoning-approved.txt").read_text() == "reasoning approval retained\n",
            "Reasoning change invalidated the pending Work approval")
    requests = [(path, body) for path, body in fixture.conversation_requests()[offset:]
                if prompt in json.dumps(body)]
    require(len(requests) >= 2, "Reasoning permission test did not complete its tool loop")
    for path, body in requests:
        require_wire_effort(path, body, "medium")
    check_reasoning_turn(app, fixture, work,
                         "fixture:chat Apply the reasoning change after approval completed.", "high")
    return work


def check_reasoning_efforts(app, fixture, directory, fixture_url, project):
    providers = []
    for protocol in ("openai-completions", "openai-responses"):
        providers.append(app.command("save_provider", provider={
            "id": "", "name": "Reasoning fixture " + protocol,
            "protocol": protocol, "baseUrl": fixture_url + "/v1",
            "models": [{"id": model, "name": model, "reasoningEffort": effort,
                        "contextWindow": 128000, "maxOutputTokens": 4096}
                       for model, effort in (("gpt-5", "low"), ("gpt-5-mini", "high"))],
        }, apiKey=FIXTURE_KEY))
    provider, responses = providers
    native_provider = copy.deepcopy(provider)
    native_provider["models"][0]["reasoningEffort"] = None
    native_provider = app.command("save_provider", provider=native_provider)
    native = app.command("create_session", projectId=None, providerId=provider["id"], modelId="gpt-5")
    offset = len(fixture.conversation_requests())
    events = app.turn(native, "fixture:chat Establish the fresh model reasoning default.")
    native_requests = fixture.conversation_requests()[offset:]
    require(bool(native_requests), "Native reasoning baseline never reached inference")
    native_effort = wire_effort(*native_requests[0])
    for path, body in native_requests:
        require_wire_effort(path, body, native_effort)
    app.wait_title(native["id"], events)
    app.command("save_provider", provider=provider)
    chat = app.command("create_session", projectId=None, providerId=provider["id"], modelId="gpt-5")
    work = app.command("create_session", projectId=project["id"], providerId=provider["id"],
                       modelId="gpt-5", reasoningEffort="high")
    require(chat["reasoningEffort"] is None, "New session copied the provider default into its override")
    require(work["reasoningEffort"] == "high", "Create session ignored the explicit reasoning override")
    for session, effort in ((chat, "low"), (work, "high")):
        events = check_reasoning_turn(app, fixture, session,
                                      "fixture:chat Preserve REASONING_ORIGINAL_HISTORY.", effort)
        app.wait_title(session["id"], events)
    chat = set_effort(app, directory, chat, "max")
    check_reasoning_turn(app, fixture, chat, "fixture:chat Use the maximum reasoning override.", "max")
    chat = set_effort(app, directory, chat, "xhigh")
    check_reasoning_turn(app, fixture, chat, "fixture:chat Preserve the raw xhigh reasoning override.", "xhigh")
    chat = set_effort(app, directory, chat, "medium")
    check_reasoning_turn(app, fixture, chat, "fixture:chat Use an explicit medium override.", "medium")
    require(stored_session(app, work["id"])["reasoningEffort"] == "high",
            "Chat reasoning change altered the Work override")
    chat = set_effort(app, directory, chat, None)
    check_reasoning_turn(app, fixture, chat, "fixture:chat Reset to the provider default.", "low")
    provider = copy.deepcopy(provider)
    provider["models"][0]["reasoningEffort"] = "medium"
    provider = app.command("save_provider", provider=provider)
    providers[0] = provider
    check_reasoning_turn(app, fixture, chat, "fixture:chat Follow the updated model default.", "medium")
    check_reasoning_turn(app, fixture, work, "fixture:chat Keep the Work override after a default change.", "high")
    provider["models"][0]["reasoningEffort"] = None
    provider = app.command("save_provider", provider=provider)
    providers[0] = provider
    check_reasoning_turn(app, fixture, chat, "fixture:chat Clear the inherited provider reasoning default.", native_effort)
    chat = set_effort(app, directory, chat, "high")
    check_reasoning_turn(app, fixture, chat, "fixture:chat Apply high before resetting to a native default.", "high")
    chat = set_effort(app, directory, chat, None)
    check_reasoning_turn(app, fixture, chat, "fixture:chat Clear the explicit override with no provider default.", native_effort)
    provider["models"][0]["reasoningEffort"] = "medium"
    provider = app.command("save_provider", provider=provider)
    providers[0] = provider
    check_reasoning_turn(app, fixture, chat, "fixture:chat Restore the provider reasoning default.", "medium")
    chat = check_active_reasoning_change(app, fixture, directory, chat, "medium", "low")
    work = check_active_reasoning_change(app, fixture, directory, work, "high", "medium")
    require(stored_session(app, chat["id"])["reasoningEffort"] == "low",
            "Work reasoning change altered the Chat override")
    work = check_pending_reasoning_change(app, fixture, directory, work)
    reject_invalid_efforts(app, directory, chat, provider, project)

    chat = set_effort(app, directory, chat, None)
    chat = switch_model(app, directory, chat, provider, "gpt-5-mini")
    check_reasoning_turn(app, fixture, chat, "fixture:chat Follow the newly selected model default.", "high")
    chat = set_effort(app, directory, chat, "medium")
    chat = switch_model(app, directory, chat, responses, "gpt-5")
    require(chat["reasoningEffort"] == "medium", "Provider change discarded the session override")
    check_reasoning_turn(app, fixture, chat, "fixture:chat Preserve the override across provider protocols.", "medium")
    chat = set_effort(app, directory, chat, None)
    check_reasoning_turn(app, fixture, chat, "fixture:chat Inherit the Responses provider default.", "low")
    chat = set_effort(app, directory, chat, "max")
    check_reasoning_turn(app, fixture, chat, "fixture:chat Keep the maximum override for restart.", "max")
    anthropic = app.command("save_provider", provider={
        "id": "", "name": "Reasoning fixture anthropic", "protocol": "anthropic",
        "baseUrl": fixture_url,
        "models": [{"id": "claude-opus-5.5", "name": "Claude Opus 5.5",
                    "contextWindow": 1000000, "maxOutputTokens": 128000,
                    "reasoningEffort": "xhigh",
                    "supportedReasoningEfforts": ["low", "medium", "high", "xhigh", "max"]}],
    }, apiKey=FIXTURE_KEY)
    for effort in (None, "xhigh"):
        session = app.command("create_session", projectId=None,
                              providerId=anthropic["id"], modelId="claude-opus-5.5",
                              reasoningEffort=effort)
        events = check_reasoning_turn(
            app, fixture, session,
            "fixture:chat Preserve the Anthropic default or explicit xhigh effort.", "xhigh",
        )
        app.wait_title(session["id"], events)
    return {"providers": providers, "chat": chat, "work": work,
            "messages": {session["id"]: app.command("messages", sessionId=session["id"])
                         for session in (chat, work)}}


def check_reasoning_after_restart(app, fixture, directory, state, ephemeral_secrets):
    if ephemeral_secrets:
        for provider in state["providers"]:
            app.command("save_provider", provider=provider, apiKey=FIXTURE_KEY)
    for name, effort in (("chat", "max"), ("work", "high")):
        original = state[name]
        restored = stored_session(app, original["id"])
        require(restored["reasoningEffort"] == effort, "Restart lost the session reasoning override")
        require(app.command("messages", sessionId=restored["id"]) == state["messages"][restored["id"]],
                "Restart changed the reasoning test conversation")
        require_saved_effort(directory, restored)
        check_reasoning_turn(app, fixture, restored,
                             "fixture:chat Resume with the saved session reasoning override.", effort)


def require_runtime_thinking(events):
    """Assert native SDK events, rather than treating an effort setting as thinking."""
    deltas = [event["data"] for event in events if event["kind"] == "reasoning_delta"]
    complete = [event["data"] for event in events if event["kind"] == "reasoning"]
    require(bool(deltas), "SDK did not expose provider thinking deltas")
    require("".join(item["content"] for item in deltas) == THINKING_TEXT,
            "SDK thinking deltas lost or duplicated provider text")
    require(bool(complete) and complete[-1]["content"] == THINKING_TEXT,
            "SDK did not expose the completed thinking text")
    require(len({item["reasoningId"] for item in deltas + complete}) == 1,
            "SDK changed the reasoning identity within one thinking block")
    require(THINKING_SIGNATURE not in json.dumps(events),
            "Opaque thinking signature entered application runtime events")


def thinking_messages(app, session):
    return [message for message in app.command("messages", sessionId=session["id"])
            if message["role"] == "reasoning"]


def wait_thinking(app, session, content):
    return app.wait_event(
        session["id"], lambda event: event["kind"] == "message"
        and event["data"].get("role") == "reasoning"
        and event["data"].get("reasoning", {}).get("status") == "running"
        and event["data"]["content"] == content,
    )["data"]


def check_thinking_text(app, fixture, provider, model, project_id=None):
    """Hold native Anthropic thinking in flight to inspect history and cancellation."""
    session = app.command("create_session", projectId=project_id,
                          providerId=provider["id"], modelId=model, reasoningEffort="high")
    offset = len(fixture.conversation_requests())
    for interrupted in (False, True):
        for gate in fixture.thinking_chunk_gates:
            gate.clear()
        prompt = ("fixture:thinking-held Cancel this thinking before its final response."
                  if interrupted else "fixture:thinking-held Stream the provider thinking before the answer.")
        app.command("send_message", sessionId=session["id"], content=prompt,
                    messageId=str(uuid.uuid4()))
        try:
            first = wait_thinking(app, session, THINKING_CHUNKS[0])
            require(first.get("sequence") is not None,
                    "First thinking chunk was emitted before it received a durable identity")
            fixture.thinking_chunk_gates[0].set()
            partial_text = "".join(THINKING_CHUNKS[:2])
            partial = wait_thinking(app, session, partial_text)
            require(partial["id"] == first["id"] and partial["sequence"] == first["sequence"],
                    "Streaming thinking changed message identity or conversation order")
            live = thinking_messages(app, session)[-1]
            require(live["id"] == first["id"] and live["content"] == partial_text
                    and live["reasoning"]["status"] == "running",
                    "History reload did not overlay the latest in-memory thinking text")
            require(THINKING_SIGNATURE not in json.dumps(live),
                    "History exposed an opaque thinking payload")
            if interrupted:
                app.command("abort_session", sessionId=session["id"])
                app.wait_idle(session["id"])
            else:
                fixture.thinking_chunk_gates[1].set()
                final_chunk = wait_thinking(app, session, THINKING_TEXT)
                require(final_chunk["id"] == first["id"],
                        "The final thinking chunk created a duplicate message")
                fixture.thinking_chunk_gates[2].set()
                answer = app.wait_event(
                    session["id"], lambda event: event["kind"] == "message"
                    and event["data"].get("role") == "assistant",
                )["data"]
                require(answer["content"] == "TANDEM_THINKING_HELD_OK",
                        "Thinking text replaced or contaminated the final answer")
                app.wait_idle(session["id"])
            saved = thinking_messages(app, session)[-1]
            require(saved["id"] == first["id"] and saved["sequence"] == first["sequence"],
                    "Thinking completion or cancellation changed its stable identity")
            require(saved["content"] == (partial_text if interrupted else THINKING_TEXT),
                    "Thinking completion or cancellation lost the received text")
            require(saved["reasoning"]["status"] == ("interrupted" if interrupted else "completed"),
                    "Thinking completion or cancellation saved the wrong status")
            if interrupted:
                require(saved.get("interrupted") is True,
                        "Canceled thinking was not marked as interrupted history")
        finally:
            for gate in fixture.thinking_chunk_gates:
                gate.set()
    messages = app.command("messages", sessionId=session["id"])
    require(len([message for message in messages if message["role"] == "reasoning"]) == 2,
            "Thinking streams were duplicated or merged across separate user turns")
    require(THINKING_SIGNATURE not in json.dumps(messages),
            "Opaque thinking signature entered persisted conversation history")
    requests = fixture.conversation_requests()[offset:]
    require(len(requests) == 2 and all(path.split("?", 1)[0].endswith("/v1/messages")
                                     for path, _ in requests),
            "Thinking acceptance did not use exactly two native Anthropic requests")
    return {"session": session, "messages": messages}


def check_thinking_after_restart(app, state):
    require(app.command("messages", sessionId=state["session"]["id"]) == state["messages"],
            "Application restart changed completed or interrupted thinking history")
