# Tandem

An agent desktop application for chat and project work. Connect your own model provider to start a conversation, or open a local or SSH directory so the agent can read files, edit code, and complete tasks in a project.

The interface uses React and the desktop shell uses Tauri 2. Rust services drive the agent through a pinned GitHub Copilot SDK. You can use your own API key without a GitHub Copilot subscription.

## Install

Download an installer from [Releases](https://github.com/caibirdme/Tandem/releases): choose `.dmg` for Mac (Apple Silicon only), or `x64-setup.exe` / `.msi` for Windows.

The macOS application is not signed or notarized by Apple. After dragging Tandem into Applications, run this in a terminal before opening it for the first time:

```sh
xattr -dr com.apple.quarantine /Applications/Tandem.app
```

## Start from source

Install Node.js 24, pnpm 12, Rust 1.99.1, just, and the [Tauri development prerequisites](https://v2.tauri.app/start/prerequisites/) for your system. On macOS, install Xcode Command Line Tools; on Windows, install Visual Studio C++ Build Tools and WebView2.

```sh
just setup       # Install frontend dependencies and nightly rustfmt.
just dev         # Open the desktop application.
```

The first Rust build downloads the SDK's matching Copilot CLI and verifies its checksum. At runtime, Tandem extracts the CLI from bundled resources; users do not need to install Copilot separately. The SDK is pinned to `v1.0.23-preview.5`, with CLI `1.0.100-1`.

The **Copilot CLI path** field in General settings is an optional debugging override. Leave it empty for normal use.

## Upgrade from Relay

Open Tandem to continue using existing projects, providers, and sessions. New installations use Tandem's application data directory and `tandem.sqlite3`. When existing Relay data is present, Tandem reads the old directory and database in place, including SDK session state. If both directories already contain databases, Tandem takes precedence; data is not merged automatically.

On Windows, you can build without an installer using `pnpm tauri build --no-bundle`, then open `target/release/tandem-desktop.exe`.

## First use

1. Open **Settings** in the lower left and add a provider. Choose OpenAI Completions, OpenAI Responses, or Anthropic, then enter its base URL and API key.
2. Add model IDs supported by that provider. Display names are customizable; set reasoning effort, context, and output limits to match each model's capabilities.
3. Create a chat, select a model, and send a message. The first message prepares the bundled runtime automatically; you do not need to click **Prepare runtime** first. Chat exposes only web reading tools and automatically approves all tool permission requests.
4. Create a project, select a local directory, and create a work session within it. Choose a permission mode beside the composer: **Ask each time** lets you allow or deny each tool permission request; **Full access** automatically approves file access, command execution, and other tool requests.

The interface starts in English. To switch to Simplified Chinese, open **Settings > General > Language**. The change takes effect immediately and is remembered on this device after restarting the application.

After a chat or work session starts, click the model name beside the composer to select any configured model, including one from another provider. The next message uses the saved selection while keeping the conversation's history, title, and tool permissions. Wait for generation or tool approval to finish, or click Stop, before switching. Unsent text stays in the composer. If a connection failure leaves the selection unconfirmed, click **Confirm model again** before sending another message.

Permission mode is saved with each work session and restored when you reopen it. You can switch after a task finishes or fails, but not during execution or while approval is pending. Every new work session starts with **Ask each time**. If this mode is still selected before the first message, a reminder explains that tool approvals may interrupt frequently and offers **Confirm and continue**, **Switch to full access and continue**, or **Cancel**. Canceling or closing the dialog preserves the draft. Local and SSH work follow the same rules. Full access is not a directory sandbox: commands can access anything the current user can access on the execution machine.

Enter an API base URL such as `https://api.example.com/v1`, without a specific `chat/completions` or `responses` endpoint. API keys are stored directly in local configuration and remain available after restarting. In the provider editor, keys are hidden by default; the eye button reveals or hides them. Save after changing a key. An empty field keeps the existing key; use **Clear saved key** to remove it.

The application does not use the system keychain. When upgrading from an early version that used a keychain, enter each API key again. The rename to Tandem preserves keys already saved in the local database.

Chat and work titles are generated after the first completed reply. In **Settings > Conversation titles**, choose a configured provider model for title generation or follow the conversation's current model. Generation runs in the background without blocking tasks; manually edited titles are preserved. For existing sessions, choose **Regenerate title** from the options menu.

## SSH projects

First log in once from a system terminal to confirm the host fingerprint and configure a key or ssh-agent. Tandem uses system OpenSSH configuration and trusted host records; it does not store login passwords.

In **New project**, select SSH, enter the host, port, user, and optional private key path, then test the connection and choose a remote directory. Tandem detects the remote platform, uploads the matching `tandem-server` to `~/.local/share/tandem/bin/`, and opens a bidirectional channel over SSH standard input/output. No additional remote network port is opened.

Hosts used before the rename also receive the new `tandem-server`. SDK state defaults to `~/.local/share/tandem/runtime`. If that directory does not exist but `~/.local/share/relay/runtime` does, Tandem uses the old state in place to continue existing sessions. It does not reuse the old `relay-server` executable.

The desktop supports macOS and Windows; remote hosts support Linux and macOS. Development builds use `just remote-build` to prepare a server for the local platform. To connect to another platform, place its binary at `src-tauri/remote/<Rust target>/tandem-server` before bundling, or set `TANDEM_REMOTE_ARTIFACTS` to an artifact directory. Release workflows build Linux/macOS servers and include them in desktop bundles. If a matching artifact is missing, the connection test reports the target platform.

## Telemetry

Enable OpenTelemetry in Settings, enter an OTLP receiver URL, and select `http/protobuf` or `http/json`. Prompt and reply content is excluded by default; enable content capture only when needed for debugging. Changes restart the runtime once active conversations stop; existing sessions can continue afterward.

Remote work exports telemetry from the remote machine, so the receiver must be reachable from that host. `localhost` refers to the machine running the agent.

## Development and verification

Tests also require Python 3.9 or later and use only its standard library.

For UI-only changes, run `just server` and `just web` in separate terminals. The former starts the local HTTP development host, `tandem-dev-server`, after building the remote agent needed for SSH. SSH uploads and desktop resources always use `tandem-server`; the development host is not distributed with the application.

```sh
just check          # Formatting, types, lint, and unit tests.
just integration    # End-to-end checks with the real SDK/CLI and controlled providers.
just desktop-build  # Bundle the desktop app and local-platform remote server.
```

[Architecture](docs/architecture.md) explains module boundaries, permissions, and localization. [Development](docs/development.md) covers quality gates, PR checks, releases, and adding a language. [Verification](docs/verification.md) distinguishes completed checks from those requiring real services.
