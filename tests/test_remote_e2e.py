"""Guard the remote acceptance harness's distinct executable roles."""

import tempfile
import unittest
from pathlib import Path

from remote_e2e import stage_binaries


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


if __name__ == "__main__":
    unittest.main()
