# Tandem user guide

Start with the [README](../README.md#get-started) to connect a provider and open your first chat or project. This guide covers the settings and controls available as you continue working.

## Configure a provider

Open **Settings > Model providers > Add provider**, choose OpenAI Chat Completions, OpenAI Responses, or Anthropic Messages, and enter the provider's base URL and API key. Add at least one model ID supported by that provider.

For each model, enter its required context window in **K** (1 K = 1,000 tokens): enter `256` for 256k or `1000` for 1M. Display names are customizable. Default reasoning effort and output limits are optional advanced settings; set them only if the model supports them. The reasoning setting supplies the default for conversations using that model; each conversation can override it.

Required fields are marked with a red **\***. If you leave one empty and click **Save provider**, the form scrolls to and focuses the first missing field and displays a prompt. Type the context limit directly into the compact input; it has no stepper arrows.

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

## Chat and project work

Choose **New chat** for a conversation with web reading tools. Chat exposes only those tools and automatically approves all tool permission requests.

To work with files, choose **Add project**, select a local or SSH directory, and create **New work** within that project. Work gives the agent file and command tools on the execution machine. The first message prepares the bundled runtime automatically; you do not need to click **Prepare runtime** first.

### Inspect tool activity

Click a progress heading in the conversation to expand the tools used for that stage. Each stage starts collapsed and shows its status and number of tool calls. Expand an individual tool to inspect its arguments, progress, result, or error. The final response stays visible below the activity groups.

Activity groups stay in conversation order when another message is sent, when you reopen the conversation, and after restarting the application. If the model does not provide a progress description, known tool types supply a short heading such as **Reading files** or **Running commands**; other combinations show **Agent activity**. Failed and interrupted activity is labeled on the collapsed group. Permission requests remain separate approval cards, so you can respond without opening tool details.

### Use project skills

In Work, type `$` to list the selected project's skills or `$prefix` to filter them by name. Select with the mouse or use the arrow keys and Enter or Tab; Escape closes the list. Work can also choose relevant skills automatically. SSH projects discover and load skills on the remote host. Chat does not load project skills.

Keep each skill's `SKILL.md` in its own subdirectory under `.agents/skills`, `.github/skills`, `.claude/skills`, or `skills` in the project. See [Project skills](skills.md) for the format and selection behavior.

### Switch models

Click the model name beside the composer to select any configured model, including one from another provider. The next message uses the saved selection while keeping the conversation's history, title, and tool permissions. Unsent text stays in the composer.

Wait for generation or tool approval to finish, or click **Stop**, before switching. If a connection failure leaves the selection unconfirmed, click **Confirm model again** before sending another message.

### Choose reasoning effort

Use **Reasoning effort** beside the composer in any chat or local/SSH work session. Choose **Low**, **Medium**, or **High** to override the model's configured default for that conversation, or choose **Default** to follow it. When the provider leaves its default unset, the model chooses its own effort. Only use effort levels supported by your model and provider.

You can choose before the first message and change it at any time, including during a reply or tool approval. Changes apply to the next message; the current response continues with its original setting. Each conversation saves its own choice, restores it when reopened, and keeps it when switching models. Choose **Default** again to follow the newly selected model's default. Changing one conversation never changes the provider or other conversations.

If a connection failure leaves the choice unconfirmed, use **Confirm reasoning effort again** before sending the next message.

### Choose tool permissions

Use the permission control beside the composer in a work session:

| Mode              | Behavior                                                                                    |
| ----------------- | ------------------------------------------------------------------------------------------- |
| **Ask each time** | Allow or deny each tool permission request. This is the default for every new work session. |
| **Full access**   | Automatically approve file access, command execution, and other tool requests.              |

Permission mode is saved with each work session and restored when you reopen it. You can switch after a task finishes or fails, but not during execution or while approval is pending. Local and SSH work follow the same rules.

Before the first message in **Ask each time**, a reminder explains that tool approvals may interrupt frequently. Choose **Confirm and continue**, **Switch to full access and continue**, or **Cancel**. Canceling or closing the dialog preserves the draft.

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
2. Choose **Add project > Remote project**. Enter the host, port, username, and optional private key path, then click **Test connection**.
3. Click the folder button (**Browse directories**) beside **Working directory**, select a remote folder, and click **Choose this directory**. Create the project, then start a work session in it.

Connection test results appear immediately below **Test connection**. Editing the host, port, username, or key path clears the previous result. Enter the port directly; its input has no stepper arrows. Required fields use the same red **\*** markers, and **Create project** focuses any missing field before submitting.

Directory browsing progress and errors stay beside the directory controls in the project dialog. On Windows, SSH runs in the background without opening a console window.

The desktop supports macOS and Windows; remote hosts support Linux and macOS. Release installers include remote agents for Linux x64 and Apple Silicon macOS. Other recognized platforms require a matching artifact; the connection test reports the target platform if one is missing.

### Remote runtime and deployment

Tandem detects the remote platform, uploads the matching `tandem-server` to `~/.local/share/tandem/bin/`, and opens a bidirectional channel over SSH standard input/output. No additional remote network port is opened. Remote SDK state is stored in `~/.local/share/tandem/runtime`.

Development builds use `just remote-build` to prepare a server for the local platform. To connect to another platform, place its binary at `src-tauri/remote/<Rust target>/tandem-server` before bundling, or set `TANDEM_REMOTE_ARTIFACTS` to an artifact directory. See [Packaging and remote artifacts](development.md#packaging-and-remote-artifacts) for build instructions and supported targets.

## Language

The interface starts in English. To switch to Simplified Chinese, open **Settings > General > Language**. The change takes effect immediately and is remembered on this device after restarting the application.

## Telemetry

Open **Settings > Observability**, enable OpenTelemetry, enter an OTLP receiver URL, and select `http/protobuf` or `http/json`. Prompt and reply content is excluded by default; enable content capture only when needed for debugging.

Changes restart the runtime once active conversations stop; existing sessions can continue afterward. Remote work exports telemetry from the remote machine, so the receiver must be reachable from that host. `localhost` refers to the machine running the agent.

## Bundled runtime

Tandem bundles the matching Copilot CLI, so you do not need to install Copilot separately. The SDK is pinned to `v1.0.23-preview.5`, with CLI `1.0.100-1`.

For source builds, the first Rust build downloads that CLI and verifies its checksum. At runtime, Tandem extracts it from bundled resources. The **Copilot CLI path** field in **Settings > General** is an optional debugging override; leave it empty for normal use.

On Windows, you can build without an installer using `pnpm tauri build --no-bundle`, then open `target/release/tandem-desktop.exe`. See [Development](development.md) for prerequisites, checks, and desktop packaging.
