#!/usr/bin/env python3
"""Deterministic acceptance against the real bundled Copilot SDK runtime.

Only inference and the OTLP collector are fixtures. The application server,
SDK process, tool execution, permission flow, persistence, and telemetry
exporter are the production code. No external API key is needed.
Chat web-tool acceptance fetches the public page https://example.com.

Run after building tandem-dev-server: python3 tests/runtime_e2e.py
"""

from __future__ import annotations

import argparse
import json
import os
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path

from acceptance_support import Application, activity_tools, debug_binary, require_context_usage
from credential_checks import require_local_secret, require_provider_key, scan_for_key
from inference_fixture import (
    FIXTURE_KEY,
    Fixture,
    handler_for,
    is_title_request,
    json_span_names,
    protobuf_span_names,
    require,
)
from permission_acceptance import auto_turn, check_chat_permissions, check_permission_modes
from model_switch_acceptance import (
    ALTERNATE_MODEL,
    check_model_switch_after_restart,
    check_model_switching,
)
from skill_acceptance import check_project_skills, check_skill_resume
from attachment_acceptance import check_attachments, check_attachment_resume
from reasoning_acceptance import (
    check_reasoning_after_restart,
    check_reasoning_efforts,
    check_thinking_after_restart,
    check_thinking_text,
)
from activity_acceptance import check_grouped_activity
from fleet_acceptance import check_fleet


def check_custom_provider_paths(app, fixture, fixture_url, project):
    for protocol, model, base_path, expected_path in (
        ("openai-completions", "gpt-4.1", "/gateway/", "/gateway/chat/completions"),
        ("openai-responses", "gpt-5", "/gateway/", "/gateway/responses"),
        ("anthropic", "claude-sonnet-4.5", "/gateway/", "/gateway/v1/messages"),
        ("anthropic", "claude-sonnet-4.5", "/gateway/v1/", "/gateway/v1/v1/messages"),
    ):
        provider = app.command(
            "save_provider",
            provider={
                "id": "", "name": "Custom path " + protocol, "protocol": protocol,
                "baseUrl": fixture_url + base_path,
                "models": [{"id": model, "name": model, "contextWindow": 256000,
                            "maxOutputTokens": 4096}],
            },
            apiKey=FIXTURE_KEY,
        )
        session = app.command("create_session", projectId=project["id"],
                              providerId=provider["id"], modelId=model)
        offset = len(fixture.conversation_requests())
        events = app.turn(session, "fixture:chat Verify the configured endpoint and context window.")
        requests = fixture.conversation_requests()[offset:]
        require(bool(requests), f"{protocol}: custom base URL did not reach inference")
        require(all(path.split("?", 1)[0] == expected_path for path, _body in requests),
                f"{protocol}: SDK request path differs from the configured base and suffix")
        usage = require_context_usage(events, 256000)
        saved = next(item for item in app.command("snapshot")["sessions"]
                     if item["id"] == session["id"])
        require(saved["contextUsage"] == usage, "Work context usage was not saved in the snapshot")
        app.wait_title(session["id"], events)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--server",
        type=Path,
        default=debug_binary("tandem-dev-server"),
        help="HTTP development host executable (tandem-dev-server)",
    )
    parser.add_argument(
        "--serve-fixture",
        action="store_true",
        help="Serve inference and OTLP for manual UI acceptance",
    )
    parser.add_argument("--port", type=int, default=4891)
    parser.add_argument(
        "--workspace", type=Path, default=Path("/tmp/tandem-ui-acceptance/project")
    )
    parser.add_argument(
        "--ephemeral-secrets",
        action="store_true",
        help="Use memory-only fixture credentials instead of verifying default local persistence",
    )
    arguments = parser.parse_args()
    if arguments.serve_fixture:
        arguments.workspace.mkdir(parents=True, exist_ok=True)
        fixture_server = ThreadingHTTPServer(
            ("127.0.0.1", arguments.port),
            handler_for(Fixture(arguments.workspace.resolve())),
        )
        print(
            f"Inference fixture: OpenAI base http://127.0.0.1:{arguments.port}/v1; "
            f"Anthropic base http://127.0.0.1:{arguments.port}; "
            f"workspace: {arguments.workspace.resolve()}",
            flush=True,
        )
        try:
            fixture_server.serve_forever()
        except KeyboardInterrupt:
            pass
        finally:
            fixture_server.server_close()
        return
    require(arguments.server.is_file(), "Build tandem-dev-server first")
    with tempfile.TemporaryDirectory(prefix="tandem-sdk-e2e-") as temporary:
        root = Path(temporary)
        workspace = root / "project"
        workspace.mkdir()
        home = root / "home"
        home.mkdir()
        environment = os.environ.copy()
        environment.update(
            HOME=str(home), USERPROFILE=str(home), XDG_CACHE_HOME=str(home / ".cache"),
            XDG_CONFIG_HOME=str(home / ".config"), XDG_STATE_HOME=str(home / ".state"),
        )
        for key in ("COPILOT_CLI_PATH", "GITHUB_TOKEN", "GH_TOKEN", "COPILOT_GITHUB_TOKEN"):
            environment.pop(key, None)
        fixture = Fixture(workspace)
        endpoint = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(fixture))
        threading.Thread(target=endpoint.serve_forever, daemon=True).start()
        fixture_url = f"http://127.0.0.1:{endpoint.server_port}"
        app = Application(arguments.server.resolve(), root, arguments.ephemeral_secrets, env=environment)
        summary = []
        providers = {}
        chats = {}
        try:
            app.command(
                "save_preferences",
                telemetry={
                    "enabled": True,
                    "endpoint": fixture_url,
                    "protocol": "http/json",
                    "captureContent": False,
                },
                cliPath=None,
            )
            for protocol, model in [
                ("openai-completions", "gpt-4.1"),
                ("openai-responses", "gpt-5"),
                ("anthropic", "claude-sonnet-4.5"),
            ]:
                provider = app.command(
                    "save_provider",
                    provider={
                        "id": "",
                        "name": "Fixture " + protocol,
                        "protocol": protocol,
                        "baseUrl": fixture_url + ("" if protocol == "anthropic" else "/v1"),
                        "models": [
                            {
                                "id": model_id,
                                "name": model_id,
                                "reasoningEffort": None,
                                "contextWindow": 128000,
                                "maxOutputTokens": 4096,
                            }
                            for model_id in (
                                [model, ALTERNATE_MODEL]
                                if protocol == "openai-completions" else [model]
                            )
                        ],
                    },
                    apiKey=FIXTURE_KEY,
                )
                providers[protocol] = (provider, model)
                require_provider_key(app, provider["id"], FIXTURE_KEY)
                if not arguments.ephemeral_secrets:
                    require_local_secret(root, provider["id"], FIXTURE_KEY)
                session = app.command(
                    "create_session",
                    projectId=None,
                    providerId=provider["id"],
                    modelId=model,
                )
                events = app.turn(
                    session, "fixture:chat Reply with the acceptance marker."
                )
                title = app.wait_title(session["id"], events)
                require(title == "Acceptance conversation summary", "Automatic chat title was not summarized")
                chats[protocol] = session
                require(
                    any(event["kind"] == "delta" for event in events),
                    f"{protocol}: streaming delta missing",
                )
                messages = app.command("messages", sessionId=session["id"])
                require(
                    any(
                        message["role"] == "assistant"
                        and "TANDEM_CHAT_OK" in message["content"]
                        for message in messages
                    ),
                    f"{protocol}: final message was not persisted",
                )
                summary.append(protocol + ": streamed and persisted")
                if protocol == "openai-completions":
                    chat_request = next(
                        body for _path, body in reversed(fixture.requests)
                        if not is_title_request(body)
                    )
                    names = [
                        tool.get("function", tool).get("name")
                        for tool in chat_request.get("tools", [])
                    ]
                    require(
                        names == ["web_fetch"],
                        f"Chat advertised unexpected tools: {names}",
                    )
                    app.turn(session, "fixture:chat Continue the same conversation.")
                    require(
                        len(app.command("messages", sessionId=session["id"])) == 4,
                        "Continued chat lost or duplicated history",
                    )
                    original = next(
                        message for message in messages if message["role"] == "user"
                    )
                    request_count = len(fixture.conversation_requests())
                    try:
                        app.command(
                            "send_message",
                            sessionId=session["id"],
                            content=original["content"],
                            messageId=original["id"],
                        )
                    except RuntimeError:
                        pass
                    else:
                        raise AssertionError(
                            "A repeated client message ID was accepted"
                        )
                    app.wait_event(
                        session["id"], lambda event: event["kind"] == "error"
                    )
                    require(
                        len(fixture.conversation_requests()) == request_count,
                        "Duplicate submission reached inference",
                    )
                    require(
                        len(app.command("messages", sessionId=session["id"])) == 4,
                        "Duplicate submission changed persisted history",
                    )
                    summary.append(
                        "message identity: repeated submission rejected before inference"
                    )
                    app.abort_streaming_turn(session, fixture)
                    app.turn(session, "fixture:chat Continue after cancellation.")
                    summary.append(
                        "cancellation: active generation aborted and conversation resumed"
                    )
                    attack = app.turn(
                        session,
                        "fixture:attack Attempt unavailable file and shell tools.",
                    )
                    require(
                        not (workspace.parent / "chat-escape.txt").exists(),
                        "Chat escaped its file capability boundary",
                    )
                    require(
                        not (workspace.parent / "chat-shell-escape.txt").exists(),
                        "Chat executed an unavailable shell tool",
                    )
                    require(
                        bool(activity_tools(attack)),
                        "Malicious tool calls were not exercised",
                    )
                    summary.append(
                        "chat boundary: malicious create and shell tool calls had no filesystem effect"
                    )
            title_provider, title_model = providers["openai-responses"]
            selection = {"providerId": title_provider["id"], "modelId": title_model}
            app.command("save_title_model", titleModel=selection)
            require(app.command("snapshot")["settings"]["titleModel"] == selection,
                    "Title model selection was not saved")
            title_chat = chats["openai-completions"]
            before_title = app.command("messages", sessionId=title_chat["id"])
            app.command("regenerate_title", sessionId=title_chat["id"])
            app.wait_title(title_chat["id"])
            title_request = next(body for _path, body in reversed(fixture.requests) if is_title_request(body))
            require(title_request["model"] == title_model, "Title did not use the selected provider/model")
            require(app.command("messages", sessionId=title_chat["id"]) == before_title,
                    "Title generation polluted conversation history")
            app.command("rename_session", sessionId=title_chat["id"], title="Manually preserved title")
            app.turn(title_chat, "fixture:chat Continue without replacing the manual title.")
            manual = next(item for item in app.command("snapshot")["sessions"] if item["id"] == title_chat["id"])
            require(manual["title"] == "Manually preserved title", "Automatic summary overwrote a manual title")
            summary.append("titles: automatic chat summary, selected provider/model, explicit regeneration, manual name preservation, and isolated history")
            provider, model = providers["openai-completions"]
            project = app.command(
                "create_project",
                name="Acceptance project",
                path=str(workspace),
                connection={"kind": "local"},
            )
            attachment_states = []
            fleet_session, fleet_history = check_fleet(app, fixture, project, provider, model, workspace)
            summary.append("fleet: explicit opt-in, two real subagents, isolated output and tool traces, durable cards, and default tools restored on the next ordinary turn")
            for attachment_provider, attachment_model in providers.values():
                attachment_states.extend(check_attachments(
                    app, fixture, project, attachment_provider, attachment_model,
                ))
            summary.append(
                "attachments: images and text reached all three protocols in Chat and Work; PDFs reached Responses and Anthropic; unsupported Completions PDFs and binary formats rejected before persistence; attachment-only sends, history metadata, and scoped retrieval"
            )
            grouped_session, grouped_history = check_grouped_activity(
                app, project, provider, model, workspace,
            )
            summary.append("grouped activity: two progress stages, parallel tools, failure visibility, preserved details, and ordered history")
            skill_state = check_project_skills(
                app, fixture, project, provider, model, workspace, home,
            )
            summary.append(
                "skills: only home/project .agents roots, automatic/explicit loading, confirmed and persisted selection metadata, plain dollar text, Chat isolation, and invalid-selection rejection"
            )
            check_custom_provider_paths(app, fixture, fixture_url, project)
            summary.append(
                "provider paths and context: custom base paths preserved for all protocols; real SDK current-window usage forwarded and persisted for work"
            )
            work = app.command(
                "create_session",
                projectId=project["id"],
                providerId=provider["id"],
                modelId=model,
            )
            create_events = app.turn(
                work,
                "fixture:work-create Create verified.txt with the requested text using the create tool.",
            )
            require_context_usage(create_events, 128000)
            require(app.wait_title(work["id"], create_events) == "Acceptance file operations", "Work title was not summarized")
            require(
                (workspace / "verified.txt").read_text() == "first version\n",
                "Real SDK create tool did not write expected file",
            )
            app.turn(
                work,
                "fixture:work-edit Replace first version with verified by SDK in verified.txt using edit.",
            )
            require(
                (workspace / "verified.txt").read_text() == "verified by SDK\n",
                "Real SDK edit tool did not update file",
            )
            shell_events = app.turn(
                work,
                "fixture:work-shell Run the shell write after reviewing its command.",
            )
            require(
                (workspace / "shell-approved.txt").read_text().strip()
                == "shell approved",
                "Approved SDK shell tool did not execute its command",
            )
            require(
                any(
                    event["kind"] == "permission"
                    and "shell-approved.txt" in event["data"]["description"]
                    and "shell approved" in event["data"]["description"]
                    for event in shell_events
                ),
                "Shell approval did not expose the command and affected file",
            )
            permission_events = app.turn(
                work,
                "fixture:permission Create the requested sibling approval file, after approval.",
            )
            require(
                (workspace.parent / "approved.txt").read_text() == "approved once\n",
                "Approved write did not complete",
            )
            require(
                any(
                    event["kind"] == "permission"
                    for event in create_events + permission_events
                ),
                "Permission callback was not exercised",
            )
            denied_events = app.turn(
                work,
                "fixture:permission-deny Request the sibling write that the user will deny.",
                approved=False,
            )
            require(
                any(event["kind"] == "permission" for event in denied_events),
                "Denied permission callback was not exercised",
            )
            require(
                not (workspace.parent / "denied.txt").exists(),
                "Denied tool still modified the filesystem",
            )
            multiple = app.turn(
                work,
                "fixture:multi-permission Perform the two independently approved writes.",
            )
            requests = {
                event["data"]["requestId"]
                for event in multiple
                if event["kind"] == "permission"
            }
            require(
                len(requests) == 2,
                "Multiple tool calls did not require distinct approvals",
            )
            require(
                all(
                    (workspace.parent / name).read_text() == "approved individually\n"
                    for name in ["multi-a.txt", "multi-b.txt"]
                ),
                "Approved parallel tool calls did not finish",
            )
            app.abort_pending_permission(work, fixture)
            app.turn(work, "fixture:chat Continue after permission cancellation.")
            summary.append(
                "local work: create/edit/shell, detailed command approval, independent approve/deny requests, and pending-permission cancellation succeeded"
            )
            automatic = check_permission_modes(app, project, provider, model, workspace)
            summary.append(
                "work permission modes: automatic read/write/edit/shell, interactive denial, busy-change rejection, and policy changes on resume succeeded"
            )
            switched_chat, automatic = check_model_switching(
                app, fixture, providers, root, automatic
            )
            summary.append(
                "model switching: same provider and cross-protocol providers retain the same session, ordered inference history, persisted selection, and work authorization; generation and pending-approval changes are rejected"
            )
            reasoning_state = check_reasoning_efforts(app, fixture, root, fixture_url, project)
            summary.append(
                "session reasoning: provider defaults, explicit overrides, reset, Chat/Work isolation, live and pending-approval changes, model/provider changes, malformed inputs, persistence, and actual OpenAI request effort verified"
            )
            thinking_provider, thinking_model = providers["anthropic"]
            thinking_state = check_thinking_text(app, fixture, thinking_provider, thinking_model)
            summary.append(
                "thinking text: native Anthropic thinking deltas, stable message identity, live history reload, opaque signature exclusion, final-answer separation, and cancellation persistence"
            )
            web_chat = app.command(
                "create_session",
                projectId=None,
                providerId=provider["id"],
                modelId=model,
            )
            check_chat_permissions(app, fixture, web_chat)
            summary.append(
                "chat permissions: real web_fetch succeeds without interactive approval"
            )
            # Restart the runtime by changing telemetry; this exercises cold SDK resume.
            before_resume = len(app.command("messages", sessionId=work["id"]))
            app.command(
                "save_preferences",
                telemetry={
                    "enabled": True,
                    "endpoint": fixture_url,
                    "protocol": "http/protobuf",
                    "captureContent": False,
                },
                cliPath=None,
            )
            app.turn(
                work, "fixture:chat Continue after restarting the Copilot runtime."
            )
            check_chat_permissions(app, fixture, web_chat)
            require(
                len(app.command("messages", sessionId=work["id"])) == before_resume + 2,
                "Cold resume duplicated or lost transcript messages",
            )
            app.command(
                "save_preferences",
                telemetry={
                    "enabled": False,
                    "endpoint": "",
                    "protocol": "http/protobuf",
                    "captureContent": False,
                },
                cliPath=None,
            )
            require(
                any(
                    path == "/v1/traces"
                    and "json" in content_type
                    and json_span_names(body)
                    for path, body, content_type in fixture.exports
                ),
                "JSON OTLP trace export missing valid spans",
            )
            require(
                any(
                    path == "/v1/traces"
                    and "protobuf" in content_type
                    and protobuf_span_names(body)
                    for path, body, content_type in fixture.exports
                ),
                "Protobuf OTLP trace export missing valid spans",
            )
            summary.append(
                "cold resume and OTLP HTTP JSON/protobuf trace export succeeded"
            )
            previous_messages = app.command("messages", sessionId=work["id"])
            previous_context = next(session["contextUsage"]
                                    for session in app.command("snapshot")["sessions"]
                                    if session["id"] == work["id"])
            cleared_provider, _ = providers["anthropic"]
            app.command("save_provider", provider=cleared_provider, apiKey="")
            require_provider_key(app, cleared_provider["id"], None)
            if not arguments.ephemeral_secrets:
                require_local_secret(root, cleared_provider["id"], None)
            app.close()
            app = Application(arguments.server.resolve(), root, arguments.ephemeral_secrets, env=environment)
            require(app.command("snapshot")["settings"]["titleModel"] == selection,
                    "Server restart lost the chosen title model")
            require(
                app.command("messages", sessionId=work["id"]) == previous_messages,
                "Server restart changed persisted messages",
            )
            require(app.command("messages", sessionId=grouped_session["id"]) == grouped_history,
                    "Server restart lost activity groups or their tool details")
            require(app.command("messages", sessionId=fleet_session["id"]) == fleet_history,
                    "Server restart lost Fleet agents, output, or tool history")
            restored_context = next(session["contextUsage"]
                                    for session in app.command("snapshot")["sessions"]
                                    if session["id"] == work["id"])
            require(restored_context == previous_context,
                    "Server restart lost the persisted context usage")
            require_provider_key(app, cleared_provider["id"], None)
            if arguments.ephemeral_secrets:
                require(
                    app.command("get_provider_api_key", id=provider["id"]) is None,
                    "Memory-only credentials unexpectedly survived a process restart",
                )
                app.command(
                    "save_provider",
                    provider=provider,
                    apiKey=FIXTURE_KEY,
                )
            else:
                require_local_secret(root, provider["id"], FIXTURE_KEY)
                require_local_secret(root, cleared_provider["id"], None)
            require_provider_key(app, provider["id"], FIXTURE_KEY)
            # Preserve credential-persistence assertions before temporarily restoring
            # every protocol's key for the attachment-history continuation checks.
            missing_attachment_keys = [
                attachment_provider
                for attachment_provider, _ in providers.values()
                if app.command("get_provider_api_key", id=attachment_provider["id"]) is None
            ]
            for attachment_provider in missing_attachment_keys:
                app.command("save_provider", provider=attachment_provider, apiKey=FIXTURE_KEY)
            try:
                check_attachment_resume(app, fixture, attachment_states)
            finally:
                for attachment_provider in missing_attachment_keys:
                    app.command("save_provider", provider=attachment_provider, apiKey="")
            summary.append("attachments after restart: original bytes, identities, and SDK image/text context retained")
            check_skill_resume(app, fixture, skill_state)
            summary.append("project skills after server restart: sent selection metadata retained and updated instructions loaded from disk")
            check_thinking_after_restart(app, thinking_state)
            summary.append("thinking text after server restart: completed and interrupted history retained")
            restored = next(
                session
                for session in app.command("snapshot")["sessions"]
                if session["id"] == automatic["id"]
            )
            require(
                restored["permissionMode"] == "allow-all",
                "Server restart lost full authorization",
            )
            require(
                restored["modelId"] == ALTERNATE_MODEL,
                "Server restart lost the switched work model",
            )
            auto_turn(
                app,
                restored,
                "fixture:work-mode-read Read after the complete server restart.",
            )
            app.turn(
                work,
                "fixture:chat Continue after restarting the entire application server.",
            )
            check_chat_permissions(app, fixture, web_chat)
            require(
                len(app.command("messages", sessionId=work["id"]))
                == len(previous_messages) + 2,
                "Server restart lost conversation continuity",
            )
            summary.append(
                "server restart: persisted conversation resumed"
                + (
                    " and local credential survived without re-entry; cleared key stayed absent"
                    if not arguments.ephemeral_secrets
                    else " (ephemeral fixture key re-provisioned)"
                )
            )
            check_model_switch_after_restart(app, fixture, root, switched_chat)
            summary.append(
                "model switching after server restart: saved model receives the complete original conversation"
            )
            check_reasoning_after_restart(app, fixture, root, reasoning_state, arguments.ephemeral_secrets)
            summary.append("session reasoning after server restart: Chat and Work overrides and complete history retained")
            if os.name != "nt":
                app.crash_runtime_during_turn(work, fixture)
                request_count = len(fixture.conversation_requests())
                app.turn(
                    work, "fixture:chat Resume only after this explicit user message."
                )
                require(
                    len(fixture.conversation_requests()) == request_count + 1,
                    "Runtime recovery replayed an earlier failed prompt",
                )
                summary.append(
                    "unexpected SDK exit: error surfaced, next user send rebuilt runtime without replay"
                )
            # Stop only this isolated test app so Windows releases SDK database
            # handles before every file is scanned. The scan exemptions stay narrow.
            app.close()
            leaks = scan_for_key(root, FIXTURE_KEY, allow_local_vault=not arguments.ephemeral_secrets)
            title_requests = [body for _path, body in fixture.requests if is_title_request(body)]
            require(title_requests and all(not body.get("tools") for body in title_requests),
                    "Title summary advertised tools or made no inference request")
            require(not leaks, f"API key leaked outside credential storage: {leaks}")
            require(
                all(
                    FIXTURE_KEY.encode() not in body
                    for _path, body, _content_type in fixture.exports
                ),
                "Provider API key leaked into OTLP export",
            )
            summary.append(
                "credential privacy: explicit key lookup works; snapshot, non-credential SQLite data, logs, SDK state, and OTLP exports contain no key"
            )
            print(
                json.dumps(
                    {
                        "result": "passed",
                        "checks": summary,
                        "inferenceRequests": len(fixture.requests),
                        "titleRequests": len(title_requests),
                        "telemetryExports": len(fixture.exports),
                    },
                    indent=2,
                )
            )
        except Exception:
            # Only fixture traffic exists here; report endpoint/schema, never auth headers.
            print(
                json.dumps(
                    {
                        "requests": [
                            {
                                "path": path,
                                "keys": list(body),
                                "tools": [
                                    tool.get("function", tool).get("name")
                                    for tool in body.get("tools", [])
                                ],
                            }
                            for path, body in fixture.requests
                        ],
                        "exports": [
                            (path, len(body), content_type)
                            for path, body, content_type in fixture.exports
                        ],
                    },
                    indent=2,
                )
            )
            print(app.log_tail())
            raise
        finally:
            fixture.release_slow_response.set()
            app.close()
            endpoint.shutdown()
            endpoint.server_close()


if __name__ == "__main__":
    main()
