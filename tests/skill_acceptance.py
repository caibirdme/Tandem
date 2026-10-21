"""Verify skill discovery and invocation through the production SDK."""

import json
import time
import uuid
from contextlib import contextmanager
from pathlib import Path

from inference_fixture import require


def write_project_skills(workspace, home):
    """Detect forbidden roots and eager loading with unique metadata/body markers."""
    sources = [
        ("auto", workspace / ".agents/skills", False, True),
        ("manual", workspace / ".agents/skills", True, True),
        ("deleted", workspace / ".agents/skills", False, True),
        ("global", home / ".agents/skills", False, True),
    ]
    for scope, directory in (("project", workspace), ("home", home)):
        for suffix, root in (
            ("github", ".github/skills"),
            ("claude", ".claude/skills"),
            ("plain", "skills"),
            ("codex", ".codex/skills"),
            ("copilot", ".copilot/skills"),
        ):
            sources.append((f"{scope}-{suffix}", directory / root, False, False))
    skills = {}
    for suffix, root, manual, available in sources:
        name = f"tandem-fixture-{suffix}"
        path = root / name / "SKILL.md"
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
            "available": available, "root": root.resolve(),
        }
    duplicate = workspace / ".github/skills/tandem-fixture-auto-duplicate/SKILL.md"
    duplicate.parent.mkdir(parents=True, exist_ok=True)
    shadowed_marker = "TANDEM_SHADOWED_SKILL_BODY_" + uuid.uuid4().hex
    shadowed_description = "Shadowed catalog marker " + uuid.uuid4().hex
    duplicate.write_text(
        f"---\nname: {skills['auto']['name']}\ndescription: {shadowed_description}\n---\n\n"
        f"This duplicate skill must not replace the allowed source: {shadowed_marker}\n",
        encoding="utf-8",
    )
    skills["auto"]["shadowed_marker"] = shadowed_marker
    skills["auto"]["shadowed_description"] = shadowed_description
    return skills


def write_global_shadow(skills, home):
    """A project definition wins over a home definition with the same name."""
    skill = skills["auto"]
    path = home / ".agents/skills/tandem-fixture-auto/SKILL.md"
    path.parent.mkdir(parents=True)
    description = "Shadowed home catalog marker " + uuid.uuid4().hex
    marker = "TANDEM_SHADOWED_HOME_SKILL_BODY_" + uuid.uuid4().hex
    path.write_text(
        f"---\nname: {skill['name']}\ndescription: {description}\n---\n\n"
        f"This home definition must not replace the project's skill: {marker}\n",
        encoding="utf-8",
    )
    skill["global_shadowed_marker"] = marker
    skill["global_shadowed_description"] = description


def check_empty_and_invalid_skill_roots(list_catalog, workspace, home):
    """Optional missing directories and unreadable definitions have no UI error."""
    require(list_catalog() == [], "Missing optional skill roots did not produce an empty catalog")
    for directory in (workspace, home):
        root = directory / ".agents/skills"
        root.parent.mkdir(parents=True, exist_ok=True)
        root.write_text("A skill root is not a directory.", encoding="utf-8")
        try:
            require(list_catalog() == [], "An invalid skill directory did not act as missing")
        finally:
            root.unlink()
        broken = root / "broken/SKILL.md"
        broken.parent.mkdir(parents=True)
        broken.write_bytes(b"\xff\xfe")
        try:
            require(list_catalog() == [], "An unreadable skill definition did not act as missing")
        finally:
            broken.unlink()
            broken.parent.rmdir()
            root.rmdir()


@contextmanager
def broken_skill_root(directory):
    """Keep valid fixtures recoverable while one allowed source cannot be scanned."""
    root = directory / ".agents/skills"
    backup = root.with_name("skills-backup")
    root.rename(backup)
    root.write_text("A skill root is not a directory.", encoding="utf-8")
    try:
        yield
    finally:
        root.unlink()
        backup.rename(root)


def skills_without_root(skills, directory):
    """Use source provenance independently of SDK canonical selection paths."""
    root = directory.resolve() / ".agents/skills"
    return {
        suffix: {**skill, "available": skill["available"] and skill["root"] != root}
        for suffix, skill in skills.items()
    }


def selection(skill):
    return {key: skill[key] for key in ("name", "path")}


def require_catalog(catalog, skills):
    for skill in skills.values():
        matches = [item for item in catalog if item["name"] == skill["name"]]
        if not skill["available"]:
            require(not matches, f"Skill from an excluded root entered the picker: {skill['name']}")
            continue
        require(len(matches) == 1, f"Allowed skill missing or duplicated: {skill['name']}")
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
    require(skills["auto"]["shadowed_description"] not in first,
            "Session catalog included the losing duplicate skill source")
    require(skills["auto"]["shadowed_marker"] not in json.dumps(requests),
            "The duplicate skill source won over the source shown in the picker")
    if "global_shadowed_description" in skills["auto"]:
        require(skills["auto"]["global_shadowed_description"] not in first,
                "The home definition replaced the project's same-name skill metadata")
        require(skills["auto"]["global_shadowed_marker"] not in json.dumps(requests),
                "The home definition replaced the project's same-name skill body")
    for suffix, skill in skills.items():
        if skill["available"] and suffix != "manual":
            require(skill["description"] in first,
                    f"Allowed skill metadata did not enter the model context: {skill['name']}")
        if not skill["available"]:
            require(skill["description"] not in first,
                    f"Excluded skill metadata entered the model context: {skill['name']}")
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


def require_selection_metadata(app, session, prompt, selected, events):
    confirmed = [event["data"] for event in events if event["kind"] == "message"
                 and event["data"].get("role") == "user"
                 and event["data"].get("content") == prompt]
    require(len(confirmed) == 1, "Skill submission did not confirm exactly one original user message")
    require(confirmed[0].get("selectedSkills", []) == selected,
            "Confirmed user event lost explicit selected-skill chip metadata")
    saved = [message for message in app.command("messages", sessionId=session["id"])
             if message["id"] == confirmed[0]["id"]]
    require(saved == confirmed, "Saved user history differs from its confirmed skill metadata")


def check_project_skills(app, fixture, project, provider, model, workspace, home):
    def list_catalog():
        return app.command("list_project_skills", projectId=project["id"])

    check_empty_and_invalid_skill_roots(list_catalog, workspace, home)
    sessions = []

    def new_session(project_id=project["id"]):
        session = app.command(
            "create_session", projectId=project_id, providerId=provider["id"], modelId=model,
        )
        sessions.append(session)
        return session

    automatic = new_session()
    app.turn(automatic, "fixture:chat Start before optional skill directories exist.")
    skills = write_project_skills(workspace, home)
    require_catalog(list_catalog(), skills)
    for directory in (workspace, home):
        with broken_skill_root(directory):
            available = skills_without_root(skills, directory)
            require_catalog(list_catalog(), available)
            offset = len(fixture.conversation_requests())
            app.turn(new_session(), "fixture:chat Continue when an optional skill root is invalid.")
            require_skill_requests(fixture, offset, available)

    write_global_shadow(skills, home)
    require_catalog(list_catalog(), skills)
    offset = len(fixture.conversation_requests())
    app.turn(automatic, "fixture:skill-auto Discover the acceptance skill added after session startup.")
    requests = require_skill_requests(fixture, offset, skills)
    require(len(requests) >= 2, "Native skill tool did not return to inference")
    require(skills["auto"]["marker"] in json.dumps(requests[-1][1]),
            "Native skill tool did not load the selected skill body")

    explicit = new_session()
    prompt = f"fixture:skill-explicit Follow ${skills['manual']['name']} selected in the composer."
    offset = len(fixture.conversation_requests())
    selected = [selection(skills["manual"])]
    events = app.turn(explicit, prompt, selected_skills=selected)
    require_selection_metadata(app, explicit, prompt, selected, events)
    require_skill_requests(fixture, offset, skills, loaded="manual")
    messages = app.command("messages", sessionId=explicit["id"])
    require([item["content"] for item in messages if item["role"] == "user"] == [prompt],
            "Skill expansion replaced the user's persisted message")
    plain_prompt = f"fixture:skill-plain Keep ${skills['manual']['name']} and $budget as ordinary text."
    events = app.turn(explicit, plain_prompt)
    require_selection_metadata(app, explicit, plain_prompt, [], events)

    multiple = new_session()
    multiple_prompt = (f"fixture:skill-multiple Combine ${skills['manual']['name']} and "
                       f"${skills['global']['name']} for this request.")
    offset = len(fixture.conversation_requests())
    selected = [
        selection(skills["manual"]), selection(skills["global"]),
    ]
    events = app.turn(multiple, multiple_prompt, selected_skills=selected)
    require_selection_metadata(app, multiple, multiple_prompt, selected, events)
    requests = require_skill_requests(fixture, offset, skills, loaded=("manual", "global"))
    first = json.dumps(requests[0][1])
    require(first.count(multiple_prompt) == 1,
            "Multiple skill selections duplicated the user's model prompt")
    require(all(first.count(skills[suffix]["marker"]) == 1 for suffix in ("manual", "global")),
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
    selected = [selection(skills["manual"])]
    events = app.turn(retried, retry_prompt, selected_skills=selected)
    require_selection_metadata(app, retried, retry_prompt, selected, events)
    require_skill_requests(fixture, offset, skills, loaded="manual")
    messages = app.command("messages", sessionId=retried["id"])
    require([item["content"] for item in messages if item["role"] == "user"] == [retry_prompt],
            "First-send retry did not preserve a single confirmed user message")
    foreign = workspace.parent / "foreign-skill.md"
    foreign.write_text("Never load a skill outside this project's catalog.", encoding="utf-8")
    rejected(new_session(), {**selection(skills["manual"]), "path": str(foreign)})
    for suffix in ("project-github", "home-copilot"):
        rejected(new_session(), selection(skills[suffix]))
    deleted = skills["deleted"]
    Path(deleted["path"]).unlink()
    deleted["available"] = False
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
    restored = app.command("messages", sessionId=session["id"])
    original = [message for message in restored if message["role"] == "user"
                and message["content"].startswith("fixture:skill-explicit")]
    require(len(original) == 1 and original[0].get("selectedSkills") == [selection(skill)],
            "Restart lost selected-skill chip metadata from the confirmed user message")
    plain = [message for message in restored if message["role"] == "user"
             and message["content"].startswith("fixture:skill-plain")]
    require(len(plain) == 1 and not plain[0].get("selectedSkills"),
            "Restart inferred a selected skill from ordinary dollar-prefixed text")
    path = Path(skill["path"])
    replacement = "TANDEM_SKILL_BODY_" + uuid.uuid4().hex
    path.write_text(path.read_text(encoding="utf-8").replace(skill["marker"], replacement),
                    encoding="utf-8")
    skill["marker"] = replacement
    offset = len(fixture.conversation_requests())
    prompt = f"fixture:skill-resume Reload ${skill['name']} after restart."
    selected = [selection(skill)]
    events = app.turn(session, prompt, selected_skills=selected)
    require_selection_metadata(app, session, prompt, selected, events)
    require(app.command("messages", sessionId=session["id"])[:len(restored)] == restored,
            "Skill resume changed earlier message identities or selected-skill metadata")
    require_skill_requests(fixture, offset, skills, loaded="manual")


def check_stdio_skills(server, fixture, options, home):
    """The real remote executable resolves paths on its execution host."""
    workspace = fixture.workspace

    def list_catalog():
        return server.command("list_project_skills", directory=str(workspace))

    check_empty_and_invalid_skill_roots(list_catalog, workspace, home)
    options = {**options, "sessionId": str(uuid.uuid4()),
               "mode": {"kind": "work", "directory": str(workspace)}}
    server.command("start_agent", options=options, resume=False)
    server.turn(options["sessionId"], "fixture:chat Start before optional remote skill directories exist.")
    skills = write_project_skills(workspace, home)
    require_catalog(list_catalog(), skills)
    for directory in (workspace, home):
        with broken_skill_root(directory):
            available = skills_without_root(skills, directory)
            require_catalog(list_catalog(), available)
            session = {**options, "sessionId": str(uuid.uuid4())}
            server.command("start_agent", options=session, resume=False)
            offset = len(fixture.conversation_requests())
            server.turn(session["sessionId"], "fixture:chat Continue when an optional remote skill root is invalid.")
            require_skill_requests(fixture, offset, available)
            server.command("disconnect_agent", sessionId=session["sessionId"])
    write_global_shadow(skills, home)
    require_catalog(list_catalog(), skills)
    offset = len(fixture.conversation_requests())
    server.turn(options["sessionId"], "fixture:skill-auto Discover the remote skill added after session startup.")
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
        selectedSkills=[selection(skills["manual"]), selection(skills["global"])],
    )
    require(len(fixture.conversation_requests()) == offset,
            "Preparing a remote skill message started inference")
    require_selection_rejected(
        lambda value: server.command(
            "send_prepared_agent_message", sessionId=explicit["sessionId"], preparedId=value,
        ), fixture, str(uuid.uuid4()),
    )
    server.prepared_turn(explicit["sessionId"], prepared_id)
    require_skill_requests(fixture, offset, skills, loaded=("manual", "global"))
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
    for suffix in ("project-github", "home-copilot"):
        require_selection_rejected(
            lambda value: server.command(
                "prepare_agent_message", sessionId=explicit["sessionId"],
                preparedId=str(uuid.uuid4()), content="fixture:skill-rejected", selectedSkills=[value],
            ), fixture, selection(skills[suffix]),
        )
    for session in (options, explicit):
        server.command("disconnect_agent", sessionId=session["sessionId"])
