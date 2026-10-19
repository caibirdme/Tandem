"""Ensure model-switch acceptance detects history loss across wire formats."""

import unittest

from model_switch_acceptance import require_wire_history


class ModelSwitchHistoryTests(unittest.TestCase):
    transcript = [
        {"role": "user", "content": 'Keep "original" history.\n保留上下文'},
        {"role": "assistant", "content": "RELAY_MODEL_SWITCH_OK old-model"},
    ]

    def test_history_content_and_roles_survive_each_supported_wire_format(self):
        for field, text_type in (
            ("messages", None),
            ("input", "input_text"),
            ("messages", "text"),
        ):
            with self.subTest(field=field, text_type=text_type):
                messages = [
                    {
                        "role": item["role"],
                        "content": (
                            item["content"] if text_type is None
                            else [{"type": text_type, "text": item["content"]}]
                        ),
                    }
                    for item in self.transcript
                ]
                require_wire_history({field: messages}, self.transcript)

    def test_missing_reordered_or_misattributed_history_cannot_pass_acceptance(self):
        for messages in (
            [],
            self.transcript[1:],
            list(reversed(self.transcript)),
            [{**item, "role": "user"} for item in self.transcript],
        ):
            with self.subTest(messages=messages):
                with self.assertRaisesRegex(AssertionError, "history"):
                    require_wire_history({"messages": messages}, self.transcript)


if __name__ == "__main__":
    unittest.main()
