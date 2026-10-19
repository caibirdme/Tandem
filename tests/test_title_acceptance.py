"""Regression checks for title-specific acceptance without external inference."""

import json
import threading
import unittest
import urllib.request
from http.server import ThreadingHTTPServer
from pathlib import Path
from unittest.mock import Mock

from acceptance_support import Application
from inference_fixture import FIXTURE_KEY, Fixture, handler_for, is_title_request


class TitleFixtureTests(unittest.TestCase):
    def test_quoted_conversation_cannot_make_the_title_fixture_execute_tools(self):
        fixture = Fixture(Path("/unused-fixture-project"))
        for key in ("messages", "input"):
            with self.subTest(protocol_field=key):
                request = {key: [{"role": "user", "content": (
                    'TANDEM_TITLE_SUMMARY\n{"firstUser":"fixture:attack; '
                    'fixture:work-create quoted conversation"}'
                )}]}
                self.assertTrue(is_title_request(request))
                self.assertEqual(fixture.response(request), "验收文件操作")
        self.assertFalse(is_title_request({"messages": [{"role": "user", "content": "normal"}]}))

    def test_title_completion_requires_a_persisted_title(self):
        for saved, expected in [("简洁标题", True), ("旧标题", False)]:
            with self.subTest(saved=saved):
                app = Mock()
                app.command.return_value = {"sessions": [{"id": "session", "title": saved}]}
                event = {"sessionId": "session", "kind": "title", "data": {"title": "简洁标题"}}
                if expected:
                    self.assertEqual(Application.wait_title(app, "session", [event]), "简洁标题")
                else:
                    with self.assertRaisesRegex(AssertionError, "persisted"):
                        Application.wait_title(app, "session", [event])

    def test_title_failure_is_not_counted_as_success(self):
        app = Mock()
        app.wait_event.return_value = {"kind": "title_error", "data": {"message": "fixture failure"}}
        with self.assertRaisesRegex(AssertionError, "Title generation failed"):
            Application.wait_title(app, "session")
        app.command.assert_not_called()


class TitleHttpFixtureTests(unittest.TestCase):
    protocols = (
        ("/v1/chat/completions", "messages", "choices", "[DONE]"),
        ("/v1/responses", "input", "output", "response.completed"),
        ("/v1/messages", "messages", "content", "message_stop"),
    )

    def setUp(self):
        self.fixture = Fixture(Path("/unused-fixture-project"))
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(self.fixture))
        self.worker = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.worker.start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.worker.join(timeout=2)

    def request_title(self, path, input_field, stream):
        body = {
            "model": "fixture-title-model",
            input_field: [{"role": "user", "content": "TANDEM_TITLE_SUMMARY\nfixture:chat"}],
        }
        if stream is not None:
            body["stream"] = stream
        request = urllib.request.Request(
            f"http://127.0.0.1:{self.server.server_port}{path}",
            json.dumps(body).encode(),
            {"Content-Type": "application/json", "Authorization": f"Bearer {FIXTURE_KEY}"},
        )
        with urllib.request.urlopen(request, timeout=3) as response:
            return response.headers.get_content_type(), response.read()

    def test_non_streaming_title_responses_use_each_protocol_json_shape(self):
        for path, input_field, output_field, _ in self.protocols:
            for stream in (None, False):
                with self.subTest(path=path, stream=stream):
                    content_type, raw = self.request_title(path, input_field, stream)
                    self.assertEqual(content_type, "application/json")
                    response = json.loads(raw)
                    self.assertEqual(response["model"], "fixture-title-model")
                    self.assertTrue(response["id"])
                    self.assertEqual(len(response[output_field]), 1)
                    if output_field == "choices":
                        self.assertEqual(response["object"], "chat.completion")
                        choice = response["choices"][0]
                        self.assertEqual(choice["finish_reason"], "stop")
                        self.assertEqual(choice["message"], {"role": "assistant", "content": "验收对话总结"})
                        self.assertEqual(response["usage"]["completion_tokens"], 10)
                    elif output_field == "output":
                        self.assertEqual(response["object"], "response")
                        self.assertEqual(response["status"], "completed")
                        item = response["output"][0]
                        self.assertEqual(item["type"], "message")
                        self.assertEqual(item["role"], "assistant")
                        self.assertEqual(item["status"], "completed")
                        self.assertEqual(item["content"], [{"type": "output_text", "text": "验收对话总结", "annotations": []}])
                        self.assertEqual(response["usage"]["output_tokens"], 10)
                    else:
                        self.assertEqual(response["type"], "message")
                        self.assertEqual(response["role"], "assistant")
                        self.assertEqual(response["stop_reason"], "end_turn")
                        self.assertEqual(response["content"], [{"type": "text", "text": "验收对话总结"}])
                        self.assertEqual(response["usage"]["output_tokens"], 10)

    def test_explicit_streaming_title_requests_keep_sse_protocol(self):
        for path, input_field, _, completed in self.protocols:
            with self.subTest(path=path):
                content_type, raw = self.request_title(path, input_field, True)
                self.assertEqual(content_type, "text/event-stream")
                self.assertIn("data: ", raw.decode())
                self.assertIn(completed, raw.decode())
                self.assertNotIn(b"\"object\": \"chat.completion\"", raw)


if __name__ == "__main__":
    unittest.main()
