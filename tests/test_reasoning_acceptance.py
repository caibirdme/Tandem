"""Keep reasoning acceptance sensitive to incorrect provider request settings."""

import unittest

from reasoning_acceptance import require_wire_effort


class ReasoningWireTests(unittest.TestCase):
    def test_explicit_and_inherited_values_use_each_provider_wire_shape(self):
        for effort in (None, "low", "medium", "high"):
            for path, body in (
                ("/v1/chat/completions", {"reasoning_effort": effort}),
                ("/v1/responses", {"reasoning": {"effort": effort}}),
            ):
                with self.subTest(path=path, effort=effort):
                    require_wire_effort(path, body, effort)
        require_wire_effort("/v1/chat/completions", {}, None)
        require_wire_effort("/v1/responses", {}, None)

    def test_absent_wrong_or_misplaced_effort_cannot_pass_acceptance(self):
        for path, body in (
            ("/v1/chat/completions", {}),
            ("/v1/chat/completions", {"reasoning_effort": "low"}),
            ("/v1/chat/completions", {"reasoning": {"effort": "high"}}),
            ("/v1/responses", {}),
            ("/v1/responses", {"reasoning": {"effort": "medium"}}),
            ("/v1/responses", {"reasoning_effort": "high"}),
        ):
            with self.subTest(path=path, body=body):
                with self.assertRaisesRegex(AssertionError, "reasoning effort"):
                    require_wire_effort(path, body, "high")


if __name__ == "__main__":
    unittest.main()
