#!/usr/bin/env python3
"""Deterministic acceptance against the real bundled Copilot SDK runtime.

Only inference and the OTLP collector are fixtures. The application server,
SDK process, tool execution, permission flow, persistence, and telemetry
exporter are the production code. No external API key is needed.

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

from acceptance_support import Application, debug_binary
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
from permission_acceptance import auto_turn, check_permission_modes
from model_switch_acceptance import (
    ALTERNATE_MODEL,
    check_model_switch_after_restart,
    check_model_switching,
)


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
            f"Inference fixture: http://127.0.0.1:{arguments.port}/v1; workspace: {arguments.workspace.resolve()}",
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
        fixture = Fixture(workspace)
        endpoint = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(fixture))
        threading.Thread(target=endpoint.serve_forever, daemon=True).start()
        fixture_url = f"http://127.0.0.1:{endpoint.server_port}"
        app = Application(arguments.server.resolve(), root, arguments.ephemeral_secrets)
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
                        "baseUrl": fixture_url + "/v1",
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
                require(title == "验收对话总结", "Automatic chat title was not summarized")
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
                        any(event["kind"] == "tool" for event in attack),
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
            app.command("rename_session", sessionId=title_chat["id"], title="手动保留的标题")
            app.turn(title_chat, "fixture:chat Continue without replacing the manual title.")
            manual = next(item for item in app.command("snapshot")["sessions"] if item["id"] == title_chat["id"])
            require(manual["title"] == "手动保留的标题", "Automatic summary overwrote a manual title")
            summary.append("titles: automatic chat summary, selected provider/model, explicit regeneration, manual name preservation, and isolated history")
            provider, model = providers["openai-completions"]
            project = app.command(
                "create_project",
                name="Acceptance project",
                path=str(workspace),
                connection={"kind": "local"},
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
            require(app.wait_title(work["id"], create_events) == "验收文件操作", "Work title was not summarized")
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
            cleared_provider, _ = providers["anthropic"]
            app.command("save_provider", provider=cleared_provider, apiKey="")
            require_provider_key(app, cleared_provider["id"], None)
            if not arguments.ephemeral_secrets:
                require_local_secret(root, cleared_provider["id"], None)
            app.close()
            app = Application(arguments.server.resolve(), root, arguments.ephemeral_secrets)
            require(app.command("snapshot")["settings"]["titleModel"] == selection,
                    "Server restart lost the chosen title model")
            require(
                app.command("messages", sessionId=work["id"]) == previous_messages,
                "Server restart changed persisted messages",
            )
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
