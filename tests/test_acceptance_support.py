"""Fast harness regression checks; no SDK process or credential store needed."""

import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from acceptance_support import Application, SessionEvents


class SessionEventsTests(unittest.TestCase):
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
