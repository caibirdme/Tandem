"""Keep steering-specific fixtures isolated from the shared acceptance flows."""

import tempfile
import unittest
from pathlib import Path

from steering_acceptance import SteeringFixture


class SteeringFixtureTests(unittest.TestCase):
    def test_shared_titles_and_unrelated_chat_keep_the_original_fixture_contract(self):
        with tempfile.TemporaryDirectory() as temporary:
            fixture = SteeringFixture(Path(temporary))
            # Active steering cases must not change another session's inference
            # or titles when this fixture hosts the entire SSH acceptance suite.
            fixture.case(tool=True)
            for prompt, expected in (
                ("TANDEM_TITLE_SUMMARY fixture:work-create Create a file.",
                 "Acceptance file operations"),
                ("TANDEM_TITLE_SUMMARY fixture:chat Hello.",
                 "Acceptance conversation summary"),
                ("fixture:chat Hello.", "TANDEM_CHAT_OK"),
            ):
                with self.subTest(prompt=prompt):
                    self.assertEqual(
                        fixture.response({"messages": [{"role": "user", "content": prompt}]}),
                        expected,
                    )


if __name__ == "__main__":
    unittest.main()
