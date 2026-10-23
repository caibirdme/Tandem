"""Local inference wire fixtures and an OTLP collector for real SDK acceptance."""

from __future__ import annotations

import gzip
import json
import shlex
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler
from pathlib import Path

from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import ExportTraceServiceRequest

FIXTURE_KEY = "fixture-only-not-a-credential"
THINKING_CHUNKS = (
    "First, I will inspect the supplied files.\n"
    "I will identify the entry point and its dependencies.\n"
    "Then I will trace the request through the application.\n"
    "I will keep the observable behavior in view.\n",
    "Next, I will compare the current behavior with the requested result.\n"
    "I will check the surrounding permission and persistence rules.\n"
    "I will account for cancellation and reconnect behavior.\n"
    "The earlier observations remain available for review.\n",
    "Finally, I will verify the result against the supplied evidence.\n"
    "I will preserve the identities of the messages and selected skills.\n"
    "I will report the completed action separately from this thinking text.\n"
    "The inspection is ready to summarize.",
)
THINKING_TEXT = "".join(THINKING_CHUNKS)
THINKING_SIGNATURE = "TANDEM_FIXTURE_OPAQUE_THINKING_SIGNATURE"


def is_title_request(body):
    return "TANDEM_TITLE_SUMMARY" in json.dumps(body, ensure_ascii=False)


class Fixture:
    def __init__(self, workspace: Path):
        self.workspace = workspace
        self.requests = []
        self.exports = []
        self.lock = threading.Lock()
        self.release_slow_response = threading.Event()
        self.thinking_chunk_gates = [threading.Event() for _ in THINKING_CHUNKS]

    def record(self, path, body, content_type):
        with self.lock:
            if path.startswith("/v1/traces") or path.startswith("/v1/metrics"):
                self.exports.append((path, body, content_type))
            else:
                self.requests.append((path, body))

    def conversation_requests(self):
        with self.lock:
            return [item for item in self.requests if not is_title_request(item[1])]

    def response(self, body):
        if is_title_request(body):
            return "Acceptance file operations" if "fixture:work" in json.dumps(body) else "Acceptance conversation summary"
        messages = body.get("messages", body.get("input", []))
        if isinstance(messages, str):
            messages = [{"role": "user", "content": messages}]
        user_indices = [
            index for index, item in enumerate(messages) if item.get("role") == "user"
        ]
        last_user = user_indices[-1] if user_indices else 0
        prompt = json.dumps(messages[last_user].get("content", "")) if messages else ""
        following = messages[last_user + 1 :]
        completed_tool = any(
            item.get("role") == "tool"
            or item.get("type") == "function_call_output"
            or any(
                part.get("type") == "tool_result"
                for part in (item.get("content") or [])
                if isinstance(part, dict)
            )
            for item in following
        )
        if "fixture:fleet-worker-" in prompt:
            worker = "first" if "fixture:fleet-worker-first" in prompt else "second"
            if not completed_tool:
                if "fleet-permission" in prompt and worker == "first":
                    return {"name": "create", "arguments": {
                        "path": str(self.workspace.parent / "fleet-denied.txt"),
                        "file_text": "must not be written\n",
                    }}
                if "fleet-cancel" in prompt:
                    names = {tool.get("function", tool).get("name") for tool in body.get("tools", [])}
                    powershell = "powershell" in names and "bash" not in names
                    return {"name": "powershell" if powershell else "bash", "arguments": {
                        "command": "Start-Sleep -Seconds 30" if powershell else "sleep 30",
                        "description": "Wait for Fleet cancellation",
                    }}
                return {"name": "view", "arguments": {
                    "path": str(self.workspace / f"fleet-{worker}.txt"),
                }}
            return "TANDEM_FLEET_WORKER_" + worker.upper() + "_OK"
        if "fixture:fleet" in prompt:
            if not completed_tool:
                return [{"name": "task", "arguments": {
                    "agent_type": "general-purpose", "mode": "sync",
                    "name": f"fleet-{worker}",
                    "description": f"Inspect the {worker} Fleet file",
                    "prompt": f"fixture:fleet-worker-{worker} Read the assigned file and report its contents."
                              + (" fleet-permission" if "fixture:fleet-permission" in prompt else "")
                              + (" fleet-cancel" if "fixture:fleet-cancel" in prompt else ""),
                }} for worker in ("first", "second")]
            return "TANDEM_FLEET_OK"
        if "fixture:thinking" in prompt:
            return "TANDEM_THINKING_HELD_OK" if "fixture:thinking-held" in prompt else "TANDEM_THINKING_OK"
        if "fixture:grouped-activity" in prompt:
            names = [
                call.get("function", {}).get("name")
                for item in following
                for call in item.get("tool_calls", [])
            ]
            if not names:
                return {"name": "report_intent", "arguments": {"intent": "Inspecting the project files"}}
            if names.count("view") == 0:
                return [
                    {"name": "view", "arguments": {"path": str(self.workspace / name)},
                     "preamble": "I will inspect the two project files."}
                    for name in ("grouped-first.txt", "grouped-second.txt")
                ]
            if names.count("report_intent") == 1:
                return {"name": "report_intent", "arguments": {"intent": "Verifying the result"}}
            if "create" not in names:
                return [
                    {"name": "create", "arguments": {
                        "path": str(self.workspace / "grouped-result.txt"), "file_text": "verified\n"},
                     "preamble": "I have inspected the files. I will verify the result."},
                    {"name": "view", "arguments": {"path": str(self.workspace / "grouped-missing.txt")}},
                ]
            return "TANDEM_GROUPED_ACTIVITY_OK"
        if not completed_tool and "fixture:skill-auto" in prompt:
            return {
                "name": "skill",
                "arguments": {"skill": "tandem-fixture-auto"},
            }
        if not completed_tool and "fixture:chat-fetch" in prompt:
            return {
                "name": "web_fetch",
                "arguments": {"url": "https://example.com"},
            }
        if not completed_tool and "fixture:attack" in prompt:
            return [
                {
                    "name": "create",
                    "arguments": {
                        "path": str(self.workspace.parent / "chat-escape.txt"),
                        "file_text": "forbidden\n",
                    },
                },
                {
                    "name": "bash",
                    "arguments": {
                        "command": f"touch {self.workspace.parent / 'chat-shell-escape.txt'}",
                        "description": "forbidden command",
                    },
                },
            ]
        if not completed_tool and "fixture:multi-permission" in prompt:
            return [
                {
                    "name": "create",
                    "arguments": {
                        "path": str(self.workspace.parent / name),
                        "file_text": "approved individually\n",
                    },
                }
                for name in ["multi-a.txt", "multi-b.txt"]
            ]
        if not completed_tool and "fixture:cancel-permission" in prompt:
            return {
                "name": "create",
                "arguments": {
                    "path": str(self.workspace.parent / "cancelled.txt"),
                    "file_text": "must not exist\n",
                },
            }
        if not completed_tool and "fixture:model-switch-permission" in prompt:
            return {
                "name": "create",
                "arguments": {
                    "path": str(self.workspace / "model-switch-approved.txt"),
                    "file_text": "model switch approval retained\n",
                },
            }
        if not completed_tool and "fixture:reasoning-permission" in prompt:
            return {
                "name": "create",
                "arguments": {
                    "path": str(self.workspace / "reasoning-approved.txt"),
                    "file_text": "reasoning approval retained\n",
                },
            }
        if "fixture:model-switch" in prompt:
            return "TANDEM_MODEL_SWITCH_OK " + body["model"]
        if "fixture:slow" in prompt:
            return "TANDEM_SLOW_OK"
        if not completed_tool and "fixture:review-commit" in prompt:
            names = {tool.get("function", tool).get("name") for tool in body.get("tools", [])}
            directory = str(self.workspace)
            if "powershell" in names and "bash" not in names:
                tool = "powershell"
                escaped = directory.replace("'", "''")
                command = (
                    f"Set-Location -LiteralPath '{escaped}'; "
                    "Set-Content -LiteralPath 'verified.txt' -Value 'committed by SDK'; "
                    "git add -- verified.txt; "
                    "git -c user.name=Acceptance -c user.email=acceptance@example.invalid "
                    "commit --only -m 'Record reviewed SDK changes' -- verified.txt"
                )
            else:
                tool = "bash"
                command = (
                    f"cd {shlex.quote(directory)} && "
                    "printf '%s\\n' 'committed by SDK' > verified.txt && "
                    "git add -- verified.txt && "
                    "git -c user.name=Acceptance -c user.email=acceptance@example.invalid "
                    "commit --only -m 'Record reviewed SDK changes' -- verified.txt"
                )
            return {"name": tool, "arguments": {
                "command": command, "description": "Edit and commit the review acceptance marker",
            }}
        if not completed_tool and any(
            tag in prompt for tag in ("fixture:work-shell", "fixture:work-mode-shell")
        ):
            names = {
                tool.get("function", tool).get("name") for tool in body.get("tools", [])
            }
            target = str(self.workspace / (
                "mode-shell.txt" if "fixture:work-mode-shell" in prompt else "shell-approved.txt"
            ))
            if "powershell" in names and "bash" not in names:
                tool = "powershell"
                escaped = target.replace("'", "''")
                command = (
                    f"Set-Content -LiteralPath '{escaped}' -Value 'shell approved'"
                )
            else:
                tool = "bash"
                command = f"printf '%s\\n' 'shell approved' > {shlex.quote(target)}"
            return {
                "name": tool,
                "arguments": {
                    "command": command,
                    "description": "Write the approved acceptance marker",
                },
            }
        if not completed_tool and "fixture:work-mode-" in prompt:
            target = str(self.workspace / "permission-mode.txt")
            for action, tool, arguments in (
                ("create", "create", {"path": target, "file_text": "automatic first\n"}),
                ("read", "view", {"path": target}),
                ("edit", "edit", {"path": target, "old_str": "automatic first", "new_str": "automatic edited"}),
                ("ask", "create", {"path": str(self.workspace / "mode-denied.txt"), "file_text": "mode changed\n"}),
            ):
                if f"fixture:work-mode-{action}" in prompt:
                    return {"name": tool, "arguments": arguments}
        if not completed_tool and "fixture:work-create" in prompt:
            return {
                "name": "create",
                "arguments": {
                    "path": str(self.workspace / "verified.txt"),
                    "file_text": "first version\n",
                },
            }
        if not completed_tool and "fixture:work-edit" in prompt:
            return {
                "name": "edit",
                "arguments": {
                    "path": str(self.workspace / "verified.txt"),
                    "old_str": "first version",
                    "new_str": "verified by SDK",
                },
            }
        if not completed_tool and "fixture:permission-deny" in prompt:
            return {
                "name": "create",
                "arguments": {
                    "path": str(self.workspace.parent / "denied.txt"),
                    "file_text": "must not exist\n",
                },
            }
        if not completed_tool and "fixture:permission" in prompt:
            return {
                "name": "create",
                "arguments": {
                    "path": str(self.workspace.parent / "approved.txt"),
                    "file_text": "approved once\n",
                },
            }
        return "TANDEM_WORK_OK" if "fixture:work" in prompt else "TANDEM_CHAT_OK"


def handler_for(fixture):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_GET(self):
            if self.path != "/__fixture/stats":
                self.send_error(404)
                return
            with fixture.lock:
                exports = [
                    {
                        "path": path,
                        "bytes": len(body),
                        "contentType": content_type,
                        "spanNames": (
                            json_span_names(body)
                            if "json" in content_type
                            else protobuf_span_names(body)
                        )
                        if path == "/v1/traces"
                        else [],
                    }
                    for path, body, content_type in fixture.exports
                ]
                data = json.dumps(
                    {"inferenceRequests": len(fixture.requests), "exports": exports}
                ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_POST(self):
            raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            if self.headers.get("Content-Encoding") == "gzip":
                raw = gzip.decompress(raw)
            content_type = self.headers.get("Content-Type", "")
            if self.path.startswith("/v1/traces") or self.path.startswith(
                "/v1/metrics"
            ):
                fixture.record(self.path, raw, content_type)
                self.send_response(200)
                self.send_header("Content-Type", content_type)
                response = b"{}" if "json" in content_type else b""
                self.send_header("Content-Length", str(len(response)))
                self.end_headers()
                self.wfile.write(response)
                return
            body = json.loads(raw)
            if (
                self.headers.get("Authorization") != f"Bearer {FIXTURE_KEY}"
                and self.headers.get("x-api-key") != FIXTURE_KEY
            ):
                self.send_error(401, "Fixture credential was not restored")
                return
            fixture.record(self.path, body, content_type)
            response = fixture.response(body)
            if body.get("stream") is not True and not isinstance(response, str):
                self.send_error(501, "Non-streaming tool fixtures are unsupported")
                return
            if self.path.endswith("/chat/completions"):
                self.completions(body, response)
            elif self.path.endswith("/responses"):
                self.openai_responses(body, response)
            elif self.path.split("?", 1)[0].endswith("/v1/messages"):
                self.anthropic(body, response)
            else:
                self.send_error(404, "Unknown inference endpoint")

        def send_json(self, value):
            data = json.dumps(value).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def start_stream(self):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.end_headers()

        def event(self, value, name=None):
            try:
                if name:
                    self.wfile.write(f"event: {name}\n".encode())
                self.wfile.write(
                    f"data: {json.dumps(value) if not isinstance(value, str) else value}\n\n".encode()
                )
                self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
                pass

        def completions(self, body, response):
            base = {
                "id": "chatcmpl-fixture",
                "object": "chat.completion.chunk",
                "created": int(time.time()),
                "model": body["model"],
            }
            if body.get("stream") is not True:
                self.send_json({
                    **base,
                    "object": "chat.completion",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": response},
                        "finish_reason": "stop",
                    }],
                    "usage": {"prompt_tokens": 100, "completion_tokens": 10, "total_tokens": 110},
                })
                return
            self.start_stream()
            if isinstance(response, (dict, list)):
                calls = response if isinstance(response, list) else [response]
                delta = {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "index": index,
                            "id": f"call_fixture_{uuid.uuid4().hex}_{index}",
                            "type": "function",
                            "function": {
                                "name": call["name"],
                                "arguments": json.dumps(call["arguments"]),
                            },
                        }
                        for index, call in enumerate(calls)
                    ],
                }
                if calls[0].get("preamble"):
                    delta["content"] = calls[0]["preamble"]
                self.event(
                    {
                        **base,
                        "choices": [
                            {"index": 0, "delta": delta, "finish_reason": None}
                        ],
                    }
                )
                finish = "tool_calls"
            else:
                for content in [response[:6], response[6:]]:
                    self.event(
                        {
                            **base,
                            "choices": [
                                {
                                    "index": 0,
                                    "delta": {"role": "assistant", "content": content},
                                    "finish_reason": None,
                                }
                            ],
                        }
                    )
                    if response == "TANDEM_SLOW_OK" and content == response[:6]:
                        fixture.release_slow_response.wait(30)
                finish = "stop"
            self.event(
                {
                    **base,
                    "choices": [{"index": 0, "delta": {}, "finish_reason": finish}],
                    "usage": {
                        "prompt_tokens": 100,
                        "completion_tokens": 10,
                        "total_tokens": 110,
                    },
                }
            )
            self.event("[DONE]")

        def openai_responses(self, body, response):
            text = response if isinstance(response, str) else "TANDEM_CHAT_OK"
            response_id = "resp_fixture"
            item = {
                "id": "msg_fixture",
                "type": "message",
                "role": "assistant",
                "status": "in_progress",
                "content": [],
            }
            envelope = {
                "id": response_id,
                "object": "response",
                "created_at": int(time.time()),
                "status": "in_progress",
                "model": body["model"],
                "output": [],
            }
            if body.get("stream") is not True:
                self.send_json({
                    **envelope,
                    "status": "completed",
                    "output": [{
                        **item,
                        "status": "completed",
                        "content": [{"type": "output_text", "text": text, "annotations": []}],
                    }],
                    "usage": {"input_tokens": 100, "output_tokens": 10, "total_tokens": 110},
                })
                return
            self.start_stream()
            self.event(
                {"type": "response.created", "response": envelope}, "response.created"
            )
            self.event(
                {"type": "response.output_item.added", "output_index": 0, "item": item},
                "response.output_item.added",
            )
            self.event(
                {
                    "type": "response.content_part.added",
                    "item_id": item["id"],
                    "output_index": 0,
                    "content_index": 0,
                    "part": {"type": "output_text", "text": "", "annotations": []},
                },
                "response.content_part.added",
            )
            for delta in [text[:6], text[6:]]:
                self.event(
                    {
                        "type": "response.output_text.delta",
                        "item_id": item["id"],
                        "output_index": 0,
                        "content_index": 0,
                        "delta": delta,
                    },
                    "response.output_text.delta",
                )
            self.event(
                {
                    "type": "response.output_text.done",
                    "item_id": item["id"],
                    "output_index": 0,
                    "content_index": 0,
                    "text": text,
                },
                "response.output_text.done",
            )
            item = {
                **item,
                "status": "completed",
                "content": [{"type": "output_text", "text": text, "annotations": []}],
            }
            self.event(
                {"type": "response.output_item.done", "output_index": 0, "item": item},
                "response.output_item.done",
            )
            envelope.update(
                status="completed",
                output=[item],
                usage={"input_tokens": 100, "output_tokens": 10, "total_tokens": 110},
            )
            self.event(
                {"type": "response.completed", "response": envelope},
                "response.completed",
            )

        def anthropic(self, body, response):
            text = response if isinstance(response, str) else "TANDEM_CHAT_OK"
            thinking = text in {"TANDEM_THINKING_OK", "TANDEM_THINKING_HELD_OK"}
            if body.get("stream") is not True:
                content = [{"type": "text", "text": text}]
                if thinking:
                    content.insert(0, {"type": "thinking", "thinking": THINKING_TEXT,
                                       "signature": THINKING_SIGNATURE})
                self.send_json({
                    "id": "msg_fixture",
                    "type": "message",
                    "role": "assistant",
                    "model": body["model"],
                    "content": content,
                    "stop_reason": "end_turn",
                    "stop_sequence": None,
                    "usage": {"input_tokens": 100, "output_tokens": 10},
                })
                return
            self.start_stream()
            self.event(
                {
                    "type": "message_start",
                    "message": {
                        "id": "msg_fixture",
                        "type": "message",
                        "role": "assistant",
                        "model": body["model"],
                        "content": [],
                        "stop_reason": None,
                        "stop_sequence": None,
                        "usage": {"input_tokens": 100, "output_tokens": 0},
                    },
                },
                "message_start",
            )
            text_index = 0
            if thinking:
                self.event({
                    "type": "content_block_start", "index": 0,
                    "content_block": {"type": "thinking", "thinking": ""},
                }, "content_block_start")
                for index, chunk in enumerate(THINKING_CHUNKS):
                    self.event({
                        "type": "content_block_delta", "index": 0,
                        "delta": {"type": "thinking_delta", "thinking": chunk},
                    }, "content_block_delta")
                    if text == "TANDEM_THINKING_HELD_OK":
                        fixture.thinking_chunk_gates[index].wait(45)
                self.event({
                    "type": "content_block_delta", "index": 0,
                    "delta": {"type": "signature_delta", "signature": THINKING_SIGNATURE},
                }, "content_block_delta")
                self.event({"type": "content_block_stop", "index": 0}, "content_block_stop")
                text_index = 1
            self.event(
                {
                    "type": "content_block_start",
                    "index": text_index,
                    "content_block": {"type": "text", "text": ""},
                },
                "content_block_start",
            )
            for delta in [text[:6], text[6:]]:
                self.event(
                    {
                        "type": "content_block_delta",
                        "index": text_index,
                        "delta": {"type": "text_delta", "text": delta},
                    },
                    "content_block_delta",
                )
            self.event({"type": "content_block_stop", "index": text_index}, "content_block_stop")
            self.event(
                {
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn", "stop_sequence": None},
                    "usage": {"output_tokens": 10},
                },
                "message_delta",
            )
            self.event({"type": "message_stop"}, "message_stop")

    return Handler


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def protobuf_span_names(payload):
    request = ExportTraceServiceRequest.FromString(payload)
    names = []
    for resource in request.resource_spans:
        for scope in resource.scope_spans:
            for span in scope.spans:
                require(len(span.trace_id) == 16, "Exported span has invalid trace ID")
                require(len(span.span_id) == 8, "Exported span has invalid span ID")
                names.append(span.name)
    return names


def json_span_names(payload):
    names = []
    for resource in json.loads(payload).get("resourceSpans", []):
        for scope in resource.get("scopeSpans", []):
            for span in scope.get("spans", []):
                require(
                    bool(span.get("traceId")) and bool(span.get("spanId")),
                    "JSON span IDs missing",
                )
                names.append(span.get("name"))
    return names
