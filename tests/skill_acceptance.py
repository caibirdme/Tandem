"""Verify skill discovery and invocation through the production SDK."""

import json
import time
import uuid
from pathlib import Path

from inference_fixture import require


def write_project_skills(workspace):
    """Use separate catalog and body markers to detect eager body loading."""
    skills = {}
    for suffix, root, manual in (
        ("auto", ".agents/skills", False),
        ("manual", ".github/skills", True),
        ("claude", ".claude/skills", False),
        ("plain", "skills", False),
    ):
        name = f"tandem-fixture-{suffix}"
        path = workspace / root / name / "SKILL.md"
        path.parent.mkdir(parents=True, exist_ok=True)
        description = f"Catalog marker {uuid.uuid4().hex} for skill acceptance."
        marker = "TANDEM_SKILL_BODY_" + uuid.uuid4().hex
        metadata = "\ndisable-model-invocation: true" if manual else ""
        path.write_text(
            f"---\nname: {name}\ndescription: {description}{metadata}\n---\n\n"
            f"When invoked, include this verification word in your answer: {marker}\n",
            encoding="utf-8",
        )
        skills[suffix] = {
            "name": name, "path": str(path), "description": description, "marker": marker,
        }
    duplicate = workspace / ".github/skills/tandem-fixture-auto-duplicate/SKILL.md"
    duplicate.parent.mkdir(parents=True, exist_ok=True)
    shadowed_marker = "TANDEM_SHADOWED_SKILL_BODY_" + uuid.uuid4().hex
    shadowed_description = "Shadowed catalog marker " + uuid.uuid4().hex
    duplicate.write_text(
        f"---\nname: {skills['auto']['name']}\ndescription: {shadowed_description}\n---\n\n"
        f"This duplicate skill must not replace the first source: {shadowed_marker}\n",
        encoding="utf-8",
    )
    skills["auto"]["shadowed_marker"] = shadowed_marker
    skills["auto"]["shadowed_description"] = shadowed_description
    return skills


def selection(skill):
    return {key: skill[key] for key in ("name", "path")}


def require_catalog(catalog, skills):
    for skill in skills.values():
        matches = [item for item in catalog if item["name"] == skill["name"]]
        require(len(matches) == 1, f"Project skill missing or duplicated: {skill['name']}")
        item = matches[0]
        require(Path(item["path"]).samefile(skill["path"]), "Skill path was not canonical")
        require(item["description"] == skill["description"], "Skill description was lost")
        skill["path"] = item["path"]
    encoded = json.dumps(catalog)
    require(all(skill["marker"] not in encoded for skill in skills.values()),
            "Skill catalog exposed full instructions instead of metadata")


def require_skill_requests(fixture, offset, skills, loaded=None):
    loaded = {loaded} if isinstance(loaded, str) else set(loaded or ())
    requests = fixture.conversation_requests()[offset:]
    require(bool(requests), "Skill acceptance did not reach the inference fixture")
    first = json.dumps(requests[0][1])
    tools = {
        tool.get("function", tool).get("name")
        for tool in requests[0][1].get("tools", [])
    }
    require("skill" in tools, "Work did not expose the native skill tool")
    require(skills["auto"]["description"] in first,
            "Automatic project skill metadata did not enter the model context")
    require(skills["auto"]["shadowed_description"] not in first,
            "Session catalog included the losing duplicate skill source")
    require(skills["auto"]["shadowed_marker"] not in json.dumps(requests),
            "The duplicate skill source won over the source shown in the picker")
    for suffix, skill in skills.items():
        if suffix not in loaded:
            require(skill["marker"] not in first,
                    f"Unselected skill instructions were loaded eagerly: {skill['name']}")
    for suffix in loaded:
        require(skills[suffix]["marker"] in first,
                "Explicit skill selection did not load its instructions before inference")
    return requests


def require_selection_rejected(send, fixture, selected):
    before = len(fixture.conversation_requests())
    try:
        send(selected)
    except RuntimeError:
        pass
    else:
        raise AssertionError("Invalid skill selection was accepted")
    require(len(fixture.conversation_requests()) == before,
            "Invalid skill selection reached inference")


def check_project_skills(app, fixture, project, provider, model, workspace):
    skills = write_project_skills(workspace)
    require_catalog(app.command("list_project_skills", projectId=project["id"]), skills)
    sessions = []

    def new_session(project_id=project["id"]):
        session = app.command(
            "create_session", projectId=project_id, providerId=provider["id"], modelId=model,
        )
        sessions.append(session)
        return session

    automatic = new_session()
    offset = len(fixture.conversation_requests())
    app.turn(automatic, "fixture:skill-auto Use the available acceptance skill.")
    requests = require_skill_requests(fixture, offset, skills)
    require(len(requests) >= 2, "Native skill tool did not return to inference")
    require(skills["auto"]["marker"] in json.dumps(requests[-1][1]),
            "Native skill tool did not load the selected skill body")

    explicit = new_session()
    prompt = "fixture:skill-explicit Follow the skill selected in the composer."
    offset = len(fixture.conversation_requests())
    app.turn(explicit, prompt, selected_skills=[selection(skills["manual"])])
    require_skill_requests(fixture, offset, skills, loaded="manual")
    messages = app.command("messages", sessionId=explicit["id"])
    require([item["content"] for item in messages if item["role"] == "user"] == [prompt],
            "Skill expansion replaced the user's persisted message")

    multiple = new_session()
    multiple_prompt = "fixture:skill-multiple Combine both selected skills for this request."
    offset = len(fixture.conversation_requests())
    app.turn(multiple, multiple_prompt, selected_skills=[
        selection(skills["manual"]), selection(skills["claude"]),
    ])
    requests = require_skill_requests(fixture, offset, skills, loaded=("manual", "claude"))
    first = json.dumps(requests[0][1])
    require(first.count(multiple_prompt) == 1,
            "Multiple skill selections duplicated the user's model prompt")
    require(all(first.count(skills[suffix]["marker"]) == 1 for suffix in ("manual", "claude")),
            "Multiple skill selections duplicated a skill body")
    messages = app.command("messages", sessionId=multiple["id"])
    require([item["content"] for item in messages if item["role"] == "user"] == [multiple_prompt],
            "Multiple skill selections duplicated the user's saved message")

    chat = new_session(None)
    offset = len(fixture.conversation_requests())
    app.turn(chat, "fixture:chat Confirm project skill isolation.")
    chat_requests = fixture.conversation_requests()[offset:]
    require(bool(chat_requests), "Chat isolation did not reach inference")
    for _, body in chat_requests:
        names = {tool.get("function", tool).get("name") for tool in body.get("tools", [])}
        require(names == {"web_fetch"}, "Chat gained project tools after skill discovery")
        encoded = json.dumps(body)
        require(all(skill["description"] not in encoded and skill["marker"] not in encoded
                    for skill in skills.values()), "Chat received project skill context")

    def rejected(session, selected):
        before = [
            item for item in app.command("messages", sessionId=session["id"])
            if item["role"] == "user"
        ]
        require_selection_rejected(
            lambda value: app.command(
                "send_message", sessionId=session["id"], content="fixture:skill-rejected",
                messageId=str(uuid.uuid4()), selectedSkills=[value],
            ),
            fixture, selected,
        )
        after = [
            item for item in app.command("messages", sessionId=session["id"])
            if item["role"] == "user"
        ]
        require(after == before, "Rejected skill selection added confirmed user history")
        if session["projectId"] is not None:
            deadline = time.monotonic() + 10
            while True:
                event = app.events.next(session["id"], deadline)
                require(not (
                    event["kind"] == "message"
                    and event["data"].get("role") == "user"
                    and event["data"].get("content") == "fixture:skill-rejected"
                ), "Rejected skill selection emitted a confirmed user message")
                if event["kind"] == "error":
                    break

    rejected(chat, selection(skills["manual"]))
    retried = new_session()
    rejected(retried, {**selection(skills["manual"]), "name": "missing-skill"})
    retry_prompt = "fixture:skill-retry Send a valid selection after the rejected first attempt."
    offset = len(fixture.conversation_requests())
    app.turn(retried, retry_prompt, selected_skills=[selection(skills["manual"])])
    require_skill_requests(fixture, offset, skills, loaded="manual")
    messages = app.command("messages", sessionId=retried["id"])
    require([item["content"] for item in messages if item["role"] == "user"] == [retry_prompt],
            "First-send retry did not preserve a single confirmed user message")
    foreign = workspace.parent / "foreign-skill.md"
    foreign.write_text("Never load a skill outside this project's catalog.", encoding="utf-8")
    rejected(new_session(), {**selection(skills["manual"]), "path": str(foreign)})
    deleted = skills["plain"]
    Path(deleted["path"]).unlink()
    rejected(multiple, selection(deleted))
    refreshed = app.command("list_project_skills", projectId=project["id"])
    require(all(item["name"] != deleted["name"] for item in refreshed),
            "Refreshing the project catalog retained a deleted skill")
    for session in sessions:
        if session["id"] != explicit["id"]:
            app.command("delete_session", sessionId=session["id"])
    return explicit, skills


def check_skill_resume(app, fixture, state):
    session, skills = state
    skill = skills["manual"]
    path = Path(skill["path"])
    replacement = "TANDEM_SKILL_BODY_" + uuid.uuid4().hex
    path.write_text(path.read_text(encoding="utf-8").replace(skill["marker"], replacement),
                    encoding="utf-8")
    skill["marker"] = replacement
    offset = len(fixture.conversation_requests())
    app.turn(session, "fixture:skill-resume Reload the selected skill after restart.",
             selected_skills=[selection(skill)])
    require_skill_requests(fixture, offset, skills, loaded="manual")


def check_stdio_skills(server, fixture, options):
    """The real remote executable resolves paths on its execution host."""
    workspace = fixture.workspace
    skills = write_project_skills(workspace)
    require_catalog(server.command("list_project_skills", directory=str(workspace)), skills)
    options = {**options, "sessionId": str(uuid.uuid4()),
               "mode": {"kind": "work", "directory": str(workspace)}}
    server.command("start_agent", options=options, resume=False)
    offset = len(fixture.conversation_requests())
    server.turn(options["sessionId"], "fixture:skill-auto Use the available remote skill.")
    requests = require_skill_requests(fixture, offset, skills)
    require(len(requests) >= 2 and skills["auto"]["marker"] in json.dumps(requests[-1][1]),
            "Remote native skill tool did not load its body")
    explicit = {**options, "sessionId": str(uuid.uuid4())}
    server.command("start_agent", options=explicit, resume=False)
    offset = len(fixture.conversation_requests())
    prepared_id = str(uuid.uuid4())
    server.command(
        "prepare_agent_message", sessionId=explicit["sessionId"], preparedId=prepared_id,
        content="fixture:skill-explicit Use the remote selected skill.",
        selectedSkills=[selection(skills["manual"])],
    )
    require(len(fixture.conversation_requests()) == offset,
            "Preparing a remote skill message started inference")
    require_selection_rejected(
        lambda value: server.command(
            "send_prepared_agent_message", sessionId=explicit["sessionId"], preparedId=value,
        ), fixture, str(uuid.uuid4()),
    )
    server.prepared_turn(explicit["sessionId"], prepared_id)
    require_skill_requests(fixture, offset, skills, loaded="manual")
    require_selection_rejected(
        lambda value: server.command(
            "send_prepared_agent_message", sessionId=explicit["sessionId"], preparedId=value,
        ), fixture, prepared_id,
    )
    require_selection_rejected(
        lambda value: server.command(
            "prepare_agent_message", sessionId=explicit["sessionId"],
            preparedId=str(uuid.uuid4()), content="fixture:skill-rejected", selectedSkills=[value],
        ), fixture, {**selection(skills["manual"]), "name": "missing-skill"},
    )
    for session in (options, explicit):
        server.command("disconnect_agent", sessionId=session["sessionId"])
