"""Fast harness regression checks; no SDK process or credential store needed."""

import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from acceptance_support import Application, SessionEvents, activity_tools


class SessionEventsTests(unittest.TestCase):
    def test_activity_upserts_keep_the_latest_tool_state_without_duplicates(self):
        events = [
            {"kind": "message", "data": {"id": "group-a", "role": "activity", "activity": {
                "tools": [{"toolCallId": "first", "status": "running"}]}}},
            {"kind": "message", "data": {"id": "answer", "role": "assistant", "content": "Done"}},
            {"kind": "message", "data": {"id": "group-a", "role": "activity", "activity": {
                "tools": [{"toolCallId": "first", "status": "error"}]}}},
            {"kind": "message", "data": {"id": "group-b", "role": "activity", "activity": {
                "tools": [{"toolCallId": "second", "status": "completed"}]}}},
        ]
        self.assertEqual(activity_tools(events), [
            {"toolCallId": "first", "status": "error"},
            {"toolCallId": "second", "status": "completed"},
        ])

    def test_interleaved_sessions_keep_their_order(self):
        inbox = SessionEvents()
        for session, sequence in [
            ("other", 1),
            ("target", 1),
            ("other", 2),
            ("target", 2),
        ]:
            inbox.put({"sessionId": session, "sequence": sequence})
        deadline = time.monotonic() + 1
        self.assertEqual(inbox.next("target", deadline)["sequence"], 1)
        self.assertEqual(inbox.next("target", deadline)["sequence"], 2)
        self.assertEqual(inbox.next("other", deadline)["sequence"], 1)
        self.assertEqual(inbox.next("other", deadline)["sequence"], 2)

    def test_expired_wait_identifies_the_session(self):
        with self.assertRaisesRegex(TimeoutError, "missing-session"):
            SessionEvents().next("missing-session", time.monotonic() - 1)

    def test_concurrent_consumers_receive_each_others_interleaved_events(self):
        inbox = SessionEvents()
        waiting = set()
        both_waiting = threading.Event()
        wait = inbox.condition.wait
        received = {}
        failures = []

        def observed_wait(timeout):
            waiting.add(threading.current_thread().name)
            if len(waiting) == 2:
                both_waiting.set()
            return wait(timeout)

        def receive(session_id):
            try:
                received[session_id] = inbox.next(session_id, time.monotonic() + 2)
            except Exception as error:
                failures.append(error)

        threads = [threading.Thread(target=receive, args=(name,), name=name)
                   for name in ("first", "second")]
        with patch.object(inbox.condition, "wait", side_effect=observed_wait):
            for thread in threads:
                thread.start()
            try:
                self.assertTrue(both_waiting.wait(1), "Both consumers must be waiting")
                inbox.put({"sessionId": "second", "sequence": 1})
                inbox.put({"sessionId": "first", "sequence": 2})
            finally:
                for thread in threads:
                    thread.join(timeout=3)
        self.assertFalse(failures)
        self.assertEqual(received["first"]["sequence"], 2)
        self.assertEqual(received["second"]["sequence"], 1)
        self.assertEqual(inbox.pending_counts(received), {"first": 0, "second": 0})


class ApplicationCleanupTests(unittest.TestCase):
    def test_local_credentials_are_default_and_memory_mode_is_explicit(self):
        for ephemeral in [False, True]:
            with (
                self.subTest(ephemeral=ephemeral),
                tempfile.TemporaryDirectory() as directory,
            ):
                root = Path(directory)
                (root / "server.log").write_text("previous process log\n")
                process = Mock()
                process.poll.return_value = 0
                with (
                    patch(
                        "acceptance_support.subprocess.Popen", return_value=process
                    ) as spawn,
                    patch.object(
                        Application,
                        "wait_ready",
                        side_effect=RuntimeError("stop before event reader"),
                    ),
                ):
                    with self.assertRaisesRegex(
                        RuntimeError, "stop before event reader"
                    ):
                        options = {"ephemeral_secrets": True} if ephemeral else {}
                        Application(
                            Path("unused-test-binary"),
                            root,
                            **options,
                        )
                arguments = spawn.call_args.args[0]
                self.assertEqual("--ephemeral-secrets" in arguments, ephemeral)
                self.assertEqual(
                    (root / "server.log").read_text(), "previous process log\n"
                )

    def test_startup_failure_stops_the_process_and_closes_its_log(self):
        process = Mock()
        process.poll.return_value = None
        process.wait.return_value = 0
        with tempfile.TemporaryDirectory() as directory:
            with (
                patch(
                    "acceptance_support.subprocess.Popen", return_value=process
                ) as spawn,
                patch.object(
                    Application,
                    "wait_ready",
                    side_effect=RuntimeError("startup failed"),
                ),
            ):
                with self.assertRaisesRegex(RuntimeError, "startup failed"):
                    Application(Path("unused-test-binary"), Path(directory))
            self.assertTrue(spawn.call_args.kwargs["stdout"].closed)
            process.wait.assert_called_once_with(timeout=20)

    def test_close_is_safe_after_an_earlier_close(self):
        process = Mock()
        process.poll.return_value = None
        process.wait.return_value = 0

        def ready(application):
            application.ready.set()

        with tempfile.TemporaryDirectory() as directory:
            with (
                patch("acceptance_support.subprocess.Popen", return_value=process),
                patch.object(Application, "wait_ready"),
                patch.object(Application, "read_events", ready),
            ):
                application = Application(Path("unused-test-binary"), Path(directory))
                application.close()
                application.close()
            self.assertTrue(application.log.closed)
            process.wait.assert_called_once_with(timeout=20)


if __name__ == "__main__":
    unittest.main()
