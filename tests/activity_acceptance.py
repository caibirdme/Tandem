"""Grouped tool activity acceptance through production local and SSH services."""

from acceptance_support import activity_tools
from inference_fixture import require


def check_grouped_activity(app, project, provider, model, workspace):
    for name in ("grouped-first.txt", "grouped-second.txt"):
        (workspace / name).write_text("Grouped activity fixture\n")
    session = app.command("create_session", projectId=project["id"],
                          providerId=provider["id"], modelId=model)
    events = app.turn(session, "fixture:grouped-activity Inspect the project files, then verify the result.")
    history = app.command("messages", sessionId=session["id"])
    groups = [message for message in history if message["role"] == "activity"]
    require(len(groups) == 2, f"Expected two meaningful activity groups, received {len(groups)}")
    require([len(group["activity"]["tools"]) for group in groups] == [2, 2],
            "Tool batches were not attached to their progress groups")
    require(groups[0]["content"] and groups[1]["content"]
            and groups[0]["content"] != groups[1]["content"],
            "Activity groups lost their distinct progress headings")
    require([group["activity"]["status"] for group in groups] == ["completed", "error"],
            "A failed tool was not reflected on its group")
    tools = activity_tools(events)
    require(len(tools) == 4 and all(tool["name"] != "report_intent" for tool in tools),
            "Internal progress reporting leaked into tool details or duplicated tools")
    require(all("arguments" in tool for tool in tools)
            and any(tool.get("result") for tool in tools if tool["status"] == "completed"),
            "Tool completion discarded its arguments or results")
    require(history[-1]["role"] == "assistant" and history[-1]["content"] == "TANDEM_GROUPED_ACTIVITY_OK",
            "The final answer was folded into an activity group")
    require([message["role"] for message in history] == ["user", "activity", "activity", "assistant"],
            "Progress text was duplicated or activity ordering was lost")
    changed = app.command("set_session_reasoning_effort", sessionId=session["id"], reasoningEffort="high")
    require(changed["reasoningEffort"] == "high", "Activity session lost its reasoning selection")
    require(app.command("messages", sessionId=session["id"]) == history,
            "Changing reasoning effort rewrote activity groups or tool details")
    app.turn(session, "fixture:chat Continue after the grouped activity.")
    continued = app.command("messages", sessionId=session["id"])
    require(continued[:len(history)] == history, "Sending another message cleared or reordered activity history")
    require((workspace / "grouped-result.txt").read_text() == "verified\n",
            "Grouped activity did not execute the real file tool")
    return session, continued
