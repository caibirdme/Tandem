"""Guard the remote acceptance harness's distinct executable roles."""

import os
import tempfile
import queue
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

from acceptance_support import SessionEvents
from inference_fixture import FIXTURE_KEY
from remote_e2e import (
    bootstrap_diagnostics, require_preparation_before_delivery,
    stage_binaries, wait_bootstrap_results,
)
from remote_protocol import private_environment


class PrivateEnvironmentTests(unittest.TestCase):
    def test_sampling_and_credentials_are_isolated_without_changing_parent_environment(self):
        inherited = {
            "OTEL_TRACES_SAMPLER": "parentbased_traceidratio",
            "OTEL_TRACES_SAMPLER_ARG": "0.01",
            "COPILOT_CLI_PATH": "fixture-cli",
            "GITHUB_TOKEN": "fixture-github-token",
            "GH_TOKEN": "fixture-gh-token",
            "COPILOT_GITHUB_TOKEN": "fixture-copilot-token",
        }
        with patch.dict(os.environ, {**inherited, "TANDEM_TEST_KEEP": "unchanged"}):
            before = dict(os.environ)
            home = Path("fixture-home")
            environment = private_environment(home)
            for key in inherited:
                self.assertNotIn(key, environment)
            self.assertEqual(environment["TANDEM_TEST_KEEP"], "unchanged")
            self.assertEqual(environment["HOME"], str(home))
            self.assertEqual(environment["USERPROFILE"], str(home))
            self.assertEqual(environment["XDG_CACHE_HOME"], str(home / ".cache"))
            self.assertEqual(environment["XDG_CONFIG_HOME"], str(home / ".config"))
            self.assertEqual(environment["XDG_STATE_HOME"], str(home / ".state"))
            self.assertEqual(dict(os.environ), before)


class BinaryRolesTests(unittest.TestCase):
    def test_staged_remote_artifact_is_independent_of_the_http_host(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            sources = []
            for role in ["remote", "dev"]:
                source = directory / role / "custom-server"
                source.parent.mkdir()
                source.write_bytes(role.encode())
                sources.append(source)
            for include_dev in [True, False]:
                with self.subTest(include_dev=include_dev):
                    staged = directory / str(include_dev)
                    staged.mkdir()
                    remote, dev = stage_binaries(
                        sources[0], sources[1] if include_dev else None, staged,
                    )
                    self.assertEqual(remote.read_bytes(), b"remote")
                    self.assertEqual(remote.stem, "tandem-server")
                    if include_dev:
                        self.assertEqual(dev.read_bytes(), b"dev")
                        self.assertEqual(dev.stem, "tandem-dev-server")
                        self.assertEqual(remote.parent, dev.parent)
                    else:
                        self.assertIsNone(dev)


class PreparationOrderTests(unittest.TestCase):
    def test_delivery_requires_both_preparation_transitions_in_order(self):
        started = {'kind': 'preparation', 'data': {'preparing': True}}
        ready = {'kind': 'preparation', 'data': {'preparing': False}}
        delivered = {'kind': 'message', 'data': {'role': 'user'}}
        require_preparation_before_delivery([started, ready, delivered])
        for events in ([delivered], [started, delivered], [ready, delivered],
                       [started, delivered, ready], [ready, started, delivered]):
            with self.subTest(events=events), self.assertRaises(AssertionError):
                require_preparation_before_delivery(events)


class ConcurrentBootstrapTests(unittest.TestCase):
    def test_workers_share_one_deadline_and_preserve_the_original_failure(self):
        results = Mock()
        results.get.side_effect = [None, None]
        with patch('remote_e2e.time.monotonic', side_effect=[10, 12, 14]):
            wait_bootstrap_results(results, 2, 10)
        self.assertEqual([call.kwargs['timeout'] for call in results.get.call_args_list], [8, 6])
        failure = RuntimeError('Worker failed during send')
        results.get.side_effect = [failure]
        with self.assertRaises(RuntimeError) as raised:
            wait_bootstrap_results(results, 1, 1)
        self.assertIs(raised.exception, failure)

    def test_missing_worker_result_reports_the_overall_deadline(self):
        with self.assertRaisesRegex(TimeoutError, 'overall|deadline'):
            wait_bootstrap_results(queue.Queue(), 1, 0)

    def test_diagnostics_include_phases_and_bounded_redacted_logs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            app = SimpleNamespace(events=SessionEvents(), reader_error=None,
                                  process=Mock(), log_path=root / 'server.log')
            app.process.poll.return_value = None
            app.events.put({'sessionId': 'first', 'kind': 'status'})
            app.log_path.write_text('x' * 9000 + FIXTURE_KEY)
            (root / 'sshd.log').write_text('fixture SSH log')
            diagnostic = bootstrap_diagnostics(app, SimpleNamespace(directory=root), {'first': 'turn'})
            self.assertIn('"first": "turn"', diagnostic)
            self.assertIn('"first": 1', diagnostic)
            self.assertIn('fixture SSH log', diagnostic)
            self.assertNotIn(FIXTURE_KEY, diagnostic)
            self.assertIn('[redacted fixture key]', diagnostic)
            self.assertLess(len(diagnostic), 9000)


if __name__ == "__main__":
    unittest.main()
