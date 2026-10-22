"""Guard the remote acceptance harness's distinct executable roles."""

import tempfile
import unittest
from pathlib import Path

from remote_e2e import require_preparation_before_delivery, stage_binaries


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


if __name__ == "__main__":
    unittest.main()
