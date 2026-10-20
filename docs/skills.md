# Project skills

Open a local or SSH project and start Work. Type `$` in the message box to see the project's skills, or type `$prefix` to filter by name. Choose a skill with the mouse, or use the arrow keys and Enter or Tab. Escape closes the list. A selected skill appears as `$name` in the message; deleting or changing that mention removes its explicit selection.

Place each skill in its own directory with a `SKILL.md` file under one of these project directories:

- `.agents/skills/`
- `.github/skills/`
- `.claude/skills/`
- `skills/`

For example, `.agents/skills/review/SKILL.md` can contain:

```markdown
---
name: review
description: Review a code change for correctness and missing verification.
---

Inspect the changed code and its callers. Report actionable problems with file
locations and explain what verification would establish the expected behavior.
```

Work makes the project skill catalog available to the agent so it can choose relevant skills automatically. Selecting a skill in the message box explicitly requests that skill for the next message. The two mechanisms use the same Copilot SDK skill support. For SSH projects, discovery and loading happen on the remote host; skill paths are remote paths.

The directories above are checked in the listed order. When names repeat, the first source wins. Skill files must resolve inside these project skill roots; ambient user skills and links to sources outside the roots are excluded. Skills marked `user-invocable: false` are omitted from the picker. Select up to 16 distinct skills per message.

Chat and automatic title generation do not load project skills. Enabling skills does not enable ambient plugins, file hooks, general configuration discovery, or change the session's permission mode.

## How explicit selection reaches the agent

Skill selection is an application/runtime protocol, rather than a model-provider parameter. The UI sends the user's text and selected skill identities separately. The backend checks identities against the active project's catalog, expands the selected skill through the runtime, and submits the resulting prompt while retaining the user's original text for display.

The implementations reviewed for this change use the following mechanisms:

| Application             | Selection and runtime behavior                                                                                                                                                                                                         |
| ----------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Codex                   | `skills/list` provides metadata for a working directory. `turn/start` accepts the text containing `$name` and a separate `{type: "skill", name, path}` input item. The server loads the full instructions.                             |
| Claude Code             | A skill's description is available for automatic matching. An explicit `/name` invocation loads the skill instructions into the conversation.                                                                                          |
| Tandem with Copilot SDK | `list_project_skills` provides project metadata. `send_message` carries `selectedSkills: [{name, path}]` alongside the original text. The runtime validates these references and uses the pinned SDK's native skill command expansion. |

Sending only `$name` leaves resolution to the agent. Tandem's picker sends an exact identity as well, so explicitly selected skills do not depend on the model guessing which file the user meant. The frontend does not read skill files or insert their bodies into system instructions.

Explicit selection supports the SDK's skill-prompt expansion. If an invocation requires another command result, a mode change, or additional runtime settings, Tandem rejects it instead of silently ignoring those requirements.

Preparation finishes before the application saves or confirms the user message. A removed or invalid skill therefore leaves the draft available for correction. For SSH, `prepare_agent_message` keeps the expanded message on the remote host and returns only an acknowledgement. `send_prepared_agent_message` consumes its session-bound identifier once. Replacing preparation, aborting, or disconnecting discards the previous prepared message.

Sources checked on October 20, 2026:

- [Codex app-server: skill inputs and listing](https://learn.chatgpt.com/docs/app-server#skills)
- [Codex: skill discovery and invocation](https://learn.chatgpt.com/docs/build-skills)
- [Claude Code: skills](https://code.claude.com/docs/en/skills)
- [Copilot SDK v1.0.23-preview.5: skills](https://github.com/github/copilot-sdk/blob/v1.0.23-preview.5/docs/features/skills.md)
- [Pinned Rust SDK configuration and message types](https://github.com/github/copilot-sdk/blob/v1.0.23-preview.5/rust/src/types.rs)
- [Pinned SDK RPC types](https://github.com/github/copilot-sdk/blob/v1.0.23-preview.5/rust/src/generated/api_types.rs)
