# Tandem user guide

Start with the [README](../README.md#get-started) to connect a provider and open your first chat or project. This guide covers the settings and controls available as you continue working.

## Configure a provider

Open **Settings > Model providers > Add provider**, choose OpenAI Chat Completions, OpenAI Responses, or Anthropic Messages, and enter the provider's base URL and API key in the compact labeled fields. Add at least one supported model through **Fetch models** or **Add model**, then click **Save provider**.

Added models appear as rows rather than expanded forms. Click a row to open its populated model form, change its values, and click **Save**. **Cancel** closes the form without changing that model. Adding, importing, editing, and removing models changes the provider draft; **Save provider** persists the complete provider configuration.

### Fetch available models

After entering the provider details, click **Fetch models** to open and load the catalog dialog. Each row shows a checkbox, display name, and context window. Expand the row's chevron to inspect its model ID, returned context and output limits, and supported reasoning efforts. All results start unchecked; select the models you want and click **Add selected** to add them to the provider's model list and close the dialog.

Tandem fills the context window from `max_input_tokens` and maximum output from `max_tokens`. For example, 1,000,000 context tokens become `1000` K in the model form, and 128,000 output tokens remain `128000`. Imported models leave default reasoning effort unset so the model chooses its own effort. Existing model IDs cannot be imported twice, and configured models remain unchanged when a fetch fails or the catalog dialog is canceled.

A catalog model needs a valid context window before it can be imported directly. Catalog entries with missing context or an output ceiling greater than or equal to the context window show why direct import is unavailable. Their returned parameters remain visible. Use **Add model** to configure such a model manually with a valid context window and an output budget smaller than that window; Tandem does not invent or silently reduce catalog limits.

**Model list path** inside the dialog defaults to `/v1/models`. It is appended to the base URL's path: `https://example.com/gateway/` becomes `https://example.com/gateway/v1/models`. Configure a different relative path when your gateway requires it, then fetch again; absolute URLs and paths that switch hosts are not accepted. Discovery always uses the [Anthropic List Models protocol](https://platform.claude.com/docs/en/api/models/list?version=2026-10-21), including API-key authentication and pagination, independently of the conversation protocol. A provider without a compatible catalog can still be configured manually.

Opening the catalog uses the current API key. Changing connection details clears earlier results, and a failed fetch can be retried in the dialog. Keys and upstream response bodies are not included in error messages. Fetching and importing alone do not save the provider.

### Add or edit a model

Click **Add model** to open an empty form, or click an added model's row to edit the same form with its saved draft values. Enter the model ID and required context window in **K** (1 K = 1,000 tokens): `256` means 256k and `1000` means 1M. The field retains single-token precision, so `131.072` means 131,072 tokens. Display names are customizable. Maximum output is optional; when specified, it must be smaller than the context window to leave room for input.

Required fields are marked with a red **\***. Save validates the model before adding or updating its row, including duplicate model IDs and token limits. Default reasoning effort is optional and starts unset for new models. The reasoning setting supplies the default for conversations using that model; each conversation can override it.

Reasoning effort values remain in English in every interface language: `low`, `medium`, `high`, `xhigh`, and `max`. When the catalog reports supported efforts, the model's default and conversation selectors offer only those values. Models reported as not supporting effort offer only the model default. Manual models and catalogs without effort information offer all five presets. The supported set is saved with the model. Changing a model ID clears that model's catalog effort restriction. Switching a conversation to a model that does not support its explicit effort resets the conversation to the new model's default.

### Check the request URL

Enter the API base URL without the request endpoint. The provider editor previews the actual request URL as you type or change protocols. Tandem trims trailing slashes and appends the endpoint for the selected protocol:

| Protocol                | Appended endpoint   |
| ----------------------- | ------------------- |
| Anthropic Messages      | `/v1/messages`      |
| OpenAI Responses        | `/responses`        |
| OpenAI Chat Completions | `/chat/completions` |

For example, Anthropic base `https://example.com/x` calls `https://example.com/x/v1/messages`. An OpenAI base ending in `/v1` retains that path. Other path segments, including an Anthropic base ending in `/v1`, are preserved exactly; check the preview against your provider's endpoint.

### Manage API keys

Open the provider editor to read or replace a saved key. Keys are hidden by default; the eye button reveals or hides them. Save after changing a key. An empty field keeps the existing key; use **Clear saved key**, then save, to remove it.

API keys are stored in plaintext in a separate configuration table in the local SQLite database and remain available after restarting. Ordinary application snapshots do not contain keys. Keys are never stored in browser storage, logs, or command-line arguments.

## Usage dashboard

Open **Settings > Usage dashboard** to inspect activity recorded on this device. Use the top-right range selector for the last **7**, **14**, or **30 days**, including today in your local time zone.

- **Daily tokens:** each bar is one day, stacked by provider and model. Hover or focus a segment for its exact count. Tokens include the SDK-reported input and output counts; cache counters are not added a second time. These cover recorded Chat and Work inference calls, including subagents; internal title generation is excluded. They are usage statistics, not a complete bill or currency estimate.
- **Daily skill use:** each line represents a skill's actual recorded invocations, not how often it appeared in a catalog. Inspect the legend to identify frequently used skills.
- **Recent sessions:** the latest ten conversations with recorded activity in the selected range show their token totals for that range, not their lifetime totals.

Counts below 1,000 are shown directly, counts from 1,000 use **K**, and counts of 1,000,000 or more use **M**. Detailed chart labels retain exact counts.

Statistics are stored locally and do not require enabling **Observability** or sending data to a collector. Local and connected SSH conversation events contribute to the same device's dashboard. Collection starts when this version first opens the local database; older conversations have no reconstructed historical token or skill counts. The dashboard displays the collection start so missing older data is not mistaken for a complete billing history. A provider or model change does not relabel usage already recorded for the original selection. Deleting a conversation removes it from the recent-session list but preserves its aggregate token and skill statistics.

## Chat and project work

Choose **New chat** for a conversation with web reading tools. Chat exposes only those tools and automatically approves all tool permission requests.

To work with files, choose **Add project**, select a local or SSH directory, and create **New work** within that project. Work gives the agent file and command tools on the execution machine. The first message prepares the bundled runtime automatically when needed. There is no manual initialization control.

### Inspect tool activity

Read the progress sentence directly in the conversation. Click the smaller category row beneath it, such as **Reading files** or **Running commands**, or its left chevron to expand the tools used for that stage. Tool details start collapsed. Running progress sentences show a moving highlight; completed sentences remain static. Expand an individual tool to inspect its arguments, progress, result, or error. The final response stays visible below the activity groups.

File reads and edits show the file name in the tool row, for example **Viewing file: app.py** or **Editing file: app.py**. Expanded arguments retain the full path. This also applies to tools inside a subagent's output panel.

Activity groups stay in conversation order when another message is sent, when you reopen the conversation, and after restarting the application. The category describes known tool types; other combinations show **Agent activity**. If the model does not provide a progress sentence, only the category row appears. Tool failure notices stay visible when the group is collapsed; detailed failures and interruptions remain inside the group, with status also exposed to assistive technology. Permission requests remain separate approval cards, so you can respond without opening tool details.

### Review code changes

In a Work conversation, click **Changes** in the header to open the file list beside the chat. Select a file to read its additions and deletions. Use **Unified** or **Side by side**, turn **Wrap lines** on when needed, and filter the loaded paths. **Load more files** retrieves another page. The panel can be resized; Escape closes it and restores focus.

Large reviews load individual diffs when you select a file. A `?` in the line totals means those counts are not available; it does not mean there were no changes or that the snapshot is incomplete.

Choose **Workspace changes** for the directory's current changes, including untracked files. **Net changes** compares HEAD with working files while retaining staged changes cancelled by a later working edit; those files can have no net text change. **Staged** compares HEAD with the index, and **Unstaged** compares the index with working files. **Since session start** compares the files just before the first submitted task with the latest captured endpoint. **This turn**, or **Review this turn** beside a user message, shows a frozen comparison for that submission. Committing files does not erase captured session or turn changes.

Review is read-only: changes are already saved in the project. Session and turn views can include edits by other sessions, editors, or tools during the same period. They cover the selected project directory; ignored files, nested repository contents, and changes elsewhere on the machine are outside coverage. Binary, oversized, conflicted, and unsupported files show an explanation instead of an incomplete text diff presented as complete. A partial capture is labeled explicitly.

Click **Refresh changes** after editing files elsewhere. Git 2.45 or newer must be installed on the project's execution host, and the directory must be inside a Git worktree. Work remains available without Git. Remote review becomes available after normal Work preparation; previously loaded historical details can be read from the desktop cache when the host is unavailable. If a starting snapshot was unavailable, use **Workspace changes** rather than assuming the session had no edits.

Workspace comparisons are unavailable when a repository assigns custom Git content filters; captured session and turn comparisons still work. Tandem reports the limitation instead of running those filters. Ordinary Git line-ending conversion is supported. Binary files and files above the review limits remain listed with an explanation; review history currently has no automatic retention cleanup.

### Read thinking text

Click **Thinking** in the conversation to expand thinking text shared by the model and provider. Consecutive thinking blocks share one header, with their text kept in order and separated by blank lines. This also applies inside a subagent's output. Replies, tool activity, and other messages start a separate section. The header starts collapsed. The expanded area shows up to three lines at a time and scrolls for longer text. While new text arrives, it follows the latest content if you are at the bottom. Scroll up to read earlier text without losing your position; scroll back to the bottom to resume following updates.

Thinking text is available only when the model and provider share it. Before text arrives, the expanded area displays a waiting message. While the model continues thinking between blocks, the same header stays active and retains the text already received. Completed thinking appears under **Thoughts**; interrupted thinking is labeled **Thinking stopped**.

### Use project skills

In Work, type `$` to list the selected project's skills or `$prefix` to filter them by name. Select with the mouse or use the arrow keys and Enter or Tab; Escape closes the list. Work can also choose relevant skills automatically. SSH projects discover and load skills on the remote host. Before a remote project has an idle, prepared conversation, its picker is empty and does not initialize the environment. Send the first task to prepare it, then reopen the picker. Chat does not load project skills.

After sending, selected skills appear as inline chips with a skill icon inside your message. The selections are saved with the message, so the chips remain after delivery is confirmed and when you reopen the conversation. Ordinary text such as `$budget` stays plain text unless you selected a matching skill from the picker.

Keep each skill's `SKILL.md` in its own subdirectory under the project's `.agents/skills` or the execution user's `~/.agents/skills`. Project definitions take precedence when names match. See [Project skills](skills.md) for the format and selection behavior.

### Run a task with multiple agents

In a local or SSH Work conversation, click **Expert team mode** in the composer before sending a task. The button starts off; its highlighted appearance indicates it is on. This choice applies to that submission. The main agent decides how to divide the work and can run independent subtasks in parallel. You do not need to define subagents first. Chat does not offer this option.

When subagents start, an **Expert team mode** card lists their names and running, completed, failed, or stopped status. Select an agent to open its output, thinking text, and tool calls inside the card. Switch between agents while they work; the main conversation stays in place. Completed agent traces remain available when reopening the conversation. **Stop** interrupts the parent task and its workers.

Subagents use the conversation's full-access tool policy. Using multiple agents may increase model usage; parallel execution does not guarantee a lower cost. Click **Expert team mode** again to turn it off before a later submission to use ordinary execution again.

### Switch models

Click the model name beside the composer to select any configured model, including one from another provider. The next message uses the saved selection while keeping the conversation's history, title, and tool permissions. Unsent text stays in the composer.

Wait for generation or tool approval to finish, or click **Stop**, before switching. If a connection failure leaves the selection unconfirmed, click **Confirm model again** before sending another message.

### Choose reasoning effort

Use **Reasoning effort** beside the composer in any chat or local/SSH work session. Choose `low`, `medium`, `high`, `xhigh`, or `max` to override the model's configured default for that conversation, or choose **Default** to follow it. These effort values remain in English in every interface language. When the catalog reports supported efforts, only those choices are offered. When the provider leaves its default unset, the model chooses its own effort.

You can choose before the first message and change it at any time, including during a reply or tool approval. Changes apply to the next message; the current response continues with its original setting. Each conversation saves its own choice and restores it when reopened. Switching models keeps a supported choice and resets an unsupported choice to **Default**. Choose **Default** again to follow the newly selected model's default. Changing one conversation never changes the provider or other conversations.

If a connection failure leaves the choice unconfirmed, use **Confirm reasoning effort again** before sending the next message.

### Full-access work

Send your task directly in a local or SSH Work conversation. Work uses **Full access**, automatically approving file access, command execution, and other tool requests. There is no permission selector or first-message confirmation dialog. The composer confirms full access with the service before sending; if that save cannot be confirmed, use **Confirm permissions again** before retrying.

Work is not a directory sandbox: commands can access anything the current user can access on the execution machine. Chat has no permission switch.

### Read context usage

In Work, hover over or focus the circular indicator in the status bar below the input to see the percentage and compact usage on one line, such as **0.5% 5.1k / 1M**.

The indicator uses the latest model runtime reading, so usage can decrease after context compaction. Before the runtime reports usage, it shows a waiting state. Readings are saved with the session and cleared when changing its model or editing its provider. Chat does not display this indicator.

### Manage conversation titles

Chat and work titles are generated after the first completed reply. In **Settings > Conversation titles**, choose a configured provider model for title generation or follow the conversation's current model.

Generation runs in the background without blocking tasks; manually edited titles are preserved. To generate another title, choose **Regenerate title** from the conversation's options menu.

## SSH projects

### Connect to a host

1. Log in once from a system terminal to confirm the host fingerprint and configure a key or ssh-agent. Tandem uses system OpenSSH configuration and trusted host records; it does not store login passwords.
2. Choose **Add project > Remote project**. Keep **From SSH config**, select a **Saved SSH host**, and click **Test connection**. The list reads `~/.ssh/config` (`%USERPROFILE%\.ssh\config` on Windows) and its included files. Use **Refresh SSH hosts** after editing the config. To enter a host, port, username, and optional private key path instead, choose **Enter manually**.
3. Click the folder button (**Browse directories**) beside **Working directory**, select a remote folder, and click **Choose this directory**. Create the project, then start a work session in it.

Saved hosts use their SSH alias directly, so OpenSSH applies the current username, port, identity files, ssh-agent, and `ProxyJump` or `ProxyCommand` configuration each time you connect. The picker lists explicit host names, not wildcard or negated patterns. Discovery follows static `Include` paths and globs; paths that require a destination such as `%h`, or another user's `~user` home, are left to OpenSSH at connection time. Missing configuration produces an empty list; manual entry remains available. Host discovery only reads files and does not execute configuration commands.

**Test connection** verifies only SSH access: it does not upload `tandem-server`, inspect its version, or initialize the runtime. Results appear immediately below **Test connection**. Choosing another host or editing connection details clears the previous result. Enter a manual port directly; its input has no stepper arrows. Required fields use the same red **\*** markers, and **Create project** focuses any missing field before submitting.

Directory browsing progress and errors stay beside the directory controls in the project dialog. On Windows, SSH runs in the background without opening a console window.

The desktop supports macOS and Windows. Both installers include the same remote server for Linux x86_64.

### Remote runtime and deployment

Send a first message in a remote Work conversation to prepare an environment that is not ready. Tandem shows **Preparing the remote environment…** while keeping the message pending. First use may take about 10 seconds; deployment, network speed, and host startup can make it longer. The task is sent only after preparation succeeds. **Stop** cancels pending preparation without sending the task. If preparation fails, correct the reported problem and retry the retained message. Ready conversations and new conversations on the same prepared host reuse the environment without a preparation notice, including after reopening the desktop app while the remote runtime remains healthy. This notice is informational, not a button. If the remote runtime has stopped, become unhealthy, or needs a matching server update, Tandem prepares it again before sending.

During preparation, Tandem detects the remote platform and uploads the matching `tandem-server` to `~/.local/share/tandem/bin/`. You can open several Work conversations on the same host. Each has an independent SSH connection while sharing one background runtime for that remote user and server build. Models, project directories, and permission decisions remain independent. No additional remote network port is opened. Remote SDK history is stored in `~/.local/share/tandem/runtime`.

If one SSH connection breaks, its work is interrupted and unanswered permissions are denied; other conversations continue. Send another message to reconnect and resume its history. Tandem does not automatically repeat a submitted task. The shared runtime stays ready for five minutes after the last connection closes, then shuts down.

When building Tandem from source, run `just desktop-build-full` initially and after remote server sources change; it prepares the Linux x86_64 server and builds the desktop for your current platform. Use `just desktop-build` for later builds; it validates the prepared server and needs no Docker. Both take no arguments and build a local executable without an installer. A full build on Mac or Windows needs a running Docker-compatible engine only when it must regenerate the server; start the engine yourself. For `just dev`, run `just remote-build` before testing SSH projects and after changing remote server sources. See [Packaging and remote artifacts](development.md#packaging-and-remote-artifacts) for setup and custom artifact directories.

## Images and files

Focus the message box and press **Ctrl+V** (**Command+V** on macOS) to paste an image from the clipboard. You can also drag files onto the composer or click its attachment button to choose files. Attachments appear as compact horizontal cards with a small image thumbnail or file icon beside the name and size. The attachment area scrolls when several rows are needed. Click an attachment's **×** to remove it before sending.

Add an optional message and send. An attachment can be sent on its own. Chat, local work, and SSH work use the same controls. For SSH work, Tandem transfers the attachment contents to the remote runtime, so the file does not need to exist on the remote machine. The selected model and provider must support the attached content.

Supported content includes PNG, JPEG, GIF, WebP, and UTF-8 text or source files. PDF documents require an OpenAI Responses or Anthropic provider; OpenAI Completions cannot deliver their contents through the pinned runtime. Each message accepts up to 10 files, at most 10 MiB per file and 20 MiB in total. Directories and unsupported binary formats are rejected with an actionable error. Failed submissions retain their attachments for retry. Sent attachments remain associated with the conversation and appear when its history is reopened; deleting the conversation deletes its stored attachment copies.

## Language

The interface starts in English. To switch to Simplified Chinese, open **Settings > General > Language**. The change takes effect immediately and is remembered on this device after restarting the application.

## Font size

Open **Settings > General > Font size** and choose **Small**, **Medium**, or **Large**. Medium is the default and makes text about 15% larger than Small; Large makes it about 30% larger. The setting scales interface and conversation text, takes effect immediately without interrupting a running conversation, and is saved locally for the next launch.

## Telemetry

Open **Settings > Observability**, enable OpenTelemetry, enter an OTLP receiver URL, and select **gRPC**, **HTTP / Protobuf**, or **HTTP / JSON**. Prompt and reply content is excluded by default; enable content capture only when needed for debugging.

For gRPC, use the Collector's gRPC address, usually `http://localhost:4317`. Use `https://collector.example:4317` for TLS with a certificate trusted by the operating system. Enter only the scheme, host, and optional port; do not append `/v1/traces` or `/v1/metrics`. For HTTP, use the base URL, usually `http://localhost:4318`; signal paths are added automatically. Enabling the Collector's OTLP gRPC receiver is required when selecting gRPC.

Changes restart the runtime once active conversations stop; existing sessions can continue afterward. Remote work exports telemetry from the remote machine, so the receiver must be reachable from that host. `localhost` refers to the machine running the agent.

To use AgentLens, select **AgentLens** as the destination, enter the collector's base URL, choose a service name such as `tandem`, and save the workspace's access key. Use the key from the destination workspace's access configuration; the key determines the workspace, so no separate workspace identifier is required. AgentLens uses HTTP/JSON and appends `/v1/traces` to the base URL. An `https://` address remains HTTPS.

Enable **Capture content** to upload complete conversation messages, system instructions, tool arguments and results, and any code or file contents included in them. With content capture disabled, only metadata is exported. AgentLens exports Chat and Work conversations, including their subagents; title generation, configuration discovery, and standalone SDK metrics are excluded. Generic OTLP export retains the SDK's native signals.

The AgentLens adapter supplies the resource attributes `service.name` and `tps.tenant.id`, translates the SDK's agent/model/tool attributes, and uses AgentLens's entry and child span kinds. The access key is stored as plaintext in a separate local SQLite configuration table. Ordinary snapshots contain no access key; the editor reads it on demand, masks it initially, and supports reveal, replacement, and clearing. Remote processes receive the key through the SSH transport and keep it in memory.

The pinned CLI omits the first model request's input for nested subagents. AgentLens still receives the actual task instruction in the parent task and subagent entry, with the latter marked as sourced from the parent task. Subsequent model requests and all supplied outputs retain their captured content. The adapter leaves the missing model input absent rather than reconstructing an unobserved request.

## Bundled runtime

Tandem bundles the matching Copilot CLI, so you do not need to install Copilot separately. The current build uses SDK `1.0.25` with CLI `1.0.102`.

For source builds, the first Rust build downloads that CLI and verifies its checksum. At runtime, Tandem extracts it from bundled resources. The **Copilot CLI path** field in **Settings > General** is an optional debugging override; leave it empty for normal use.

On Windows, install just and Git for Windows, then run `just desktop-build-full` in Git Bash for the first local build. Open `target/release/tandem-desktop.exe`. Use `just desktop-build` while the remote server artifact remains current. Release installers come from GitHub CI; `just desktop-bundle` is available for local packaging checks. See [Development](development.md) for prerequisites and verification.
