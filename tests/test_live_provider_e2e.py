"""Live-provider harness policy checks: no real credentials or network calls."""

import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from live_provider_e2e import (
    ANTHROPIC_OUTPUT_LIMIT,
    MODELS,
    Observations,
    WorkPolicy,
    check_chat,
    redact,
    run_turn,
    scan_for_key,
)


class SafetyTests(unittest.TestCase):
    def test_approval_allowlist_and_redaction(self):
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            policy = WorkPolicy(workspace, "python -I /private/verify.py")
            for request, expected in [
                ({"kind": "write", "details": {"fileName": "result.txt"}}, True),
                ({"kind": "read", "details": {"path": "seed.json"}}, True),
                ({"kind": "write", "details": {"path": "../outside"}}, False),
                ({"kind": "write", "details": {"path": "verify.py"}}, False),
                (
                    {"kind": "shell", "details": {"fullCommandText": policy.command}},
                    True,
                ),
                (
                    {
                        "kind": "shell",
                        "details": {"fullCommandText": policy.command + "; rm -rf /"},
                    },
                    False,
                ),
                ({"kind": "url", "details": {"url": "https://example.com"}}, False),
                ({"kind": "future", "details": {}}, False),
            ]:
                self.assertEqual(policy.permits(request), expected)
            self.assertEqual(
                redact({"error": "key=private"}, "private"), {"error": "key=[redacted]"}
            )

    def test_request_observation_does_not_rewrite_bytes(self):
        body = b'{ "model": "claude-haiku-5.5", "max_tokens": 128000, "messages": [] }'
        original = bytes(body)
        observations = Observations(MODELS)
        record = observations.record("/pingress/v1/messages", body)
        self.assertEqual(body, original)
        self.assertEqual(record["max_tokens"], ANTHROPIC_OUTPUT_LIMIT)
        self.assertNotIn("messages", record)

    def test_credential_scan_reports_paths_only(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "data").mkdir()
            (root / "data" / "example").write_bytes(b"x" * 65533 + b"fixture-only-key")
            (root / "server.log").write_text("clean")
            self.assertEqual(scan_for_key(root, "fixture-only-key"), ["data/example"])


class CancellationTests(unittest.TestCase):
    def test_user_echo_cannot_satisfy_the_assistant_nonce_check(self):
        events = [
            {
                "kind": "message",
                "data": {"role": "user", "content": "Reply with TANDEM_CHAT_known"},
            },
            {
                "kind": "message",
                "data": {"role": "assistant", "content": "Wrong answer"},
            },
        ]
        with (
            patch("live_provider_e2e.secrets.token_hex", return_value="known"),
            patch("live_provider_e2e.run_turn", return_value=events),
            self.assertRaisesRegex(AssertionError, "unpredictable nonce"),
        ):
            check_chat(Mock(), {"id": "provider"}, MODELS[0], 120)

    def test_failed_turn_cancels_even_when_cleanup_also_fails(self):
        app = Mock()
        app.events.next.side_effect = TimeoutError("original turn timeout")
        app.command.side_effect = [None, RuntimeError("cleanup failure")]
        with self.assertRaisesRegex(TimeoutError, "original turn timeout"):
            run_turn(app, {"id": "session"}, "prompt", None, 120)
        self.assertEqual(app.command.call_args.args, ("abort_session",))
        self.assertEqual(app.command.call_args.kwargs, {"sessionId": "session"})


if __name__ == "__main__":
    unittest.main()
