"""Keep permission acceptance strict and preserve tool failure diagnostics."""

import unittest
from unittest.mock import Mock

from permission_acceptance import auto_turn


def tool_event(status, error=None):
    return {
        "kind": "message",
        "data": {
            "id": "activity",
            "role": "activity",
            "activity": {"tools": [{
                "name": "web_fetch",
                "status": status,
                "error": error,
            }]},
        },
    }


class AutomaticPermissionTests(unittest.TestCase):
    def test_tool_failure_reports_the_underlying_error(self):
        app = Mock()
        app.turn.return_value = [tool_event(
            "error", {"code": "failure", "message": "fixture connection failed"}
        )]
        with self.assertRaisesRegex(
            AssertionError, 'web_fetch.*fixture connection failed'
        ):
            auto_turn(app, {"id": "session"}, "fixture prompt")
        app.turn.assert_called_once_with(
            {"id": "session"}, "fixture prompt", approved=False
        )

    def test_interactive_permission_still_fails_acceptance(self):
        app = Mock()
        app.turn.return_value = [{"kind": "permission"}, tool_event("completed")]
        with self.assertRaisesRegex(AssertionError, "interactive approval"):
            auto_turn(app, {"id": "session"}, "fixture prompt")

    def test_successful_tool_execution_preserves_events(self):
        app = Mock()
        app.turn.return_value = [tool_event("running"), tool_event("completed")]
        self.assertIs(
            auto_turn(app, {"id": "session"}, "fixture prompt"),
            app.turn.return_value,
        )


if __name__ == "__main__":
    unittest.main()
