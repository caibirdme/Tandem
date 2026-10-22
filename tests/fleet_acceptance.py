"""Fleet acceptance through production services and the real Copilot runtime."""

import uuid

from inference_fixture import require


FLEET_TOOLS = {"task", "read_agent", "write_agent", "list_agents", "sql"}


def tool_names(body):
    return {tool.get("function", tool).get("name") for tool in body.get("tools", [])}


def check_fleet(app, fixture, project, provider, model, workspace):
    for worker in ("first", "second"):
        (workspace / f"fleet-{worker}.txt").write_text(f"{worker} worker evidence\n")
    session = app.command("create_session", projectId=project["id"],
                          providerId=provider["id"], modelId=model)
    offset = len(fixture.conversation_requests())
    app.turn(session, "fixture:chat Verify ordinary execution first.")
    ordinary = fixture.conversation_requests()[offset:]
    require(ordinary and all(not (tool_names(body) & FLEET_TOOLS) for _path, body in ordinary),
            "Ordinary Work advertised Fleet tools without an explicit opt-in")

    offset = len(fixture.conversation_requests())
    events = app.turn(session, "fixture:fleet Inspect both files independently, then summarize.", fleet=True)
    fleet_requests = fixture.conversation_requests()[offset:]
    require(any("task" in tool_names(body) for _path, body in fleet_requests),
            "Fleet did not advertise its delegation tool")
    history = app.command("messages", sessionId=session["id"])
    agents = [message["agent"] for message in history if message["role"] == "agent"]
    require(len(agents) == 2, f"Expected two durable Fleet agents, received {agents}")
    require(all(agent["status"] == "completed" for agent in agents),
            f"Fleet agents did not complete: {agents}")
    require(len({agent["id"] for agent in agents}) == 2, "Fleet agents shared one identity")
    for worker, marker in (("first", "FIRST"), ("second", "SECOND")):
        outputs = [message for message in history if message["role"] == "assistant"
                   and message["content"] == f"TANDEM_FLEET_WORKER_{marker}_OK"]
        require(len(outputs) == 1 and outputs[0].get("agentId"),
                f"The {worker} worker output lost its agent identity: {outputs}")
        agent_id = outputs[0]["agentId"]
        require(any(agent["id"] == agent_id for agent in agents),
                f"Worker output did not match an agent card: {agent_id}")
        traces = [message for message in history if message.get("agentId") == agent_id
                  and message["role"] == "activity"]
        require(any(tool["name"] == "view" and tool["status"] == "completed"
                    and f"fleet-{worker}.txt" in str(tool.get("arguments"))
                    for message in traces for tool in message["activity"]["tools"]),
                f"The {worker} worker's file tool did not stay in its trace")
    require(any(event["kind"] == "message" and event["data"].get("agent", {}).get("status") == "running"
                for event in events), "Fleet did not stream running worker cards")
    require(any(event["kind"] == "message" and event["data"].get("agentId") for event in events),
            "Worker output was not forwarded while the parent turn was active")
    require(any(event["kind"] == "delta" and event["data"].get("agentId") for event in events),
            "Worker streaming deltas were not forwarded with their identity")
    require(history[-1]["role"] == "assistant" and history[-1]["content"] == "TANDEM_FLEET_OK"
            and not history[-1].get("agentId"), "Fleet did not preserve a separate main answer")

    offset = len(fixture.conversation_requests())
    app.turn(session, "fixture:chat Return to ordinary execution.")
    resumed = fixture.conversation_requests()[offset:]
    require(resumed and all(not (tool_names(body) & FLEET_TOOLS) for _path, body in resumed),
            "Fleet tools remained enabled after the option was cleared")
    require(app.command("messages", sessionId=session["id"])[:len(history)] == history,
            "A later ordinary turn rewrote Fleet agent history")
    app.command("set_permission_mode", sessionId=session["id"], permissionMode="ask")
    denied = app.turn(session, "fixture:fleet-permission Deny the worker's write.", approved=False, fleet=True)
    require(any(event["kind"] == "permission" for event in denied),
            "A worker write did not use the parent permission handler")
    require(not (workspace.parent / "fleet-denied.txt").exists(),
            "A worker bypassed a denied permission")

    app.command("set_permission_mode", sessionId=session["id"], permissionMode="allow-all")
    app.command("send_message", sessionId=session["id"],
                content="fixture:fleet-cancel Stop the workers while commands are running.",
                messageId=str(uuid.uuid4()), fleet=True)
    active = app.wait_event(session["id"], lambda event: event["kind"] == "message"
                           and event["data"].get("agentId")
                           and any(tool["name"] in {"bash", "powershell"} and tool["status"] == "running"
                                   for tool in event["data"].get("activity", {}).get("tools", [])))
    require(active["data"]["agentId"], "Running worker command had no identity")
    app.command("abort_session", sessionId=session["id"])
    app.wait_idle(session["id"])
    cancelled = app.command("messages", sessionId=session["id"])
    require(any(message.get("agent", {}).get("status") == "interrupted" for message in cancelled),
            "Stopped Fleet workers were not marked interrupted")
    require(not any(message.get("agent", {}).get("status") == "running"
                    or message.get("activity", {}).get("status") == "running" for message in cancelled),
            "Stopped Fleet retained running agents or tool indicators")
    return session, app.command("messages", sessionId=session["id"])


def check_stdio_fleet(server, fixture, options):
    for worker in ("first", "second"):
        (fixture.workspace / f"fleet-{worker}.txt").write_text(f"{worker} worker evidence\n")
    session_id = str(uuid.uuid4())
    work = {**options, "sessionId": session_id,
            "mode": {"kind": "work", "directory": str(fixture.workspace)}}
    server.command("start_agent", options=work, resume=False)
    events = server.turn(session_id, "fixture:fleet Inspect both files independently.", fleet=True)
    started = [event["data"] for event in events
               if event["kind"] == "agent" and event["data"].get("event") == "subagent.started"]
    require(len(started) == 2, f"Remote Fleet did not start two workers: {started}")
    worker_ids = {event["data"]["agentId"] for event in events
                  if event["kind"] == "message" and event["data"].get("agentId")}
    require(len(worker_ids) == 2, "Remote worker messages lost their separate identities")
    require(any(event["kind"] == "tool" and event["data"].get("agentId") for event in events),
            "Remote worker tools lost their identity")
    offset = len(fixture.conversation_requests())
    server.turn(session_id, "fixture:chat Return to ordinary remote execution.")
    require(all(not (tool_names(body) & FLEET_TOOLS)
                for _path, body in fixture.conversation_requests()[offset:]),
            "Remote Fleet tools remained available after opt-out")
    server.command("disconnect_agent", sessionId=session_id)
