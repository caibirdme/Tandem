# Tandem

An agent desktop for chat, local projects, and SSH work. Bring your own model provider and API key to ask questions, explore a codebase, edit files, and run project tasks.

No GitHub Copilot subscription is required. Available for Windows and Apple Silicon Mac.

[Download](https://github.com/caibirdme/Tandem/releases) · [Get started](#get-started) · [User guide](docs/user-guide.md) · [Development](docs/development.md)

![A Tandem work session showing a project conversation and model selection](docs/images/work-session.jpg)

_Keep project conversations and model selection together in one workspace. Screenshots use illustrative data and may show an earlier interface._

## What you can do

- **Chat with your models.** Connect OpenAI-compatible APIs, Anthropic, or your own model gateway. Chat can read web pages.
- **Include images and files.** Paste an image or drop files into the composer, review the previews, and send them with your message.
- **Work where your files are.** Open a local directory or connect to a Linux or macOS host over SSH. The agent can read and edit files and run commands.
- **Choose how the agent works.** Work uses full access automatically. Switch models and choose Low, Medium, High, or Max reasoning effort for each conversation.
- **Delegate independent work.** Select **Expert team mode** in the Work composer to let the main agent coordinate subagents. Open an agent from its collaboration card to inspect its output and tool calls.

## Install

Download an installer from [Releases](https://github.com/caibirdme/Tandem/releases):

| System                | Installer       |
| --------------------- | --------------- |
| macOS (Apple Silicon) | `.dmg`          |
| Windows (x64)         | `x64-setup.exe` |

On macOS, drag Tandem into Applications. The app is not signed or notarized by Apple; before opening it for the first time, run:

```sh
xattr -dr com.apple.quarantine /Applications/Tandem.app
```

## Get started

### 1. Connect a model

Open **Settings > Model providers > Add provider**. Choose OpenAI Chat Completions, OpenAI Responses, or Anthropic Messages, then enter your provider's base URL and API key.

Add a model ID supported by that provider and its required context window in **K** (1 K = 1,000 tokens). For example, enter `256` for 256k or `1000` for 1M. Save the provider.

![Tandem provider settings with a request URL preview, API key field, model ID, and required context window](docs/images/provider-settings.jpg)

_Use your provider's model IDs and check the request URL preview before saving._

### 2. Start a conversation

- **For questions:** choose **New chat**, select a model, and send a message. Chat exposes web reading tools and approves their permission requests automatically.
- **For project tasks:** choose **Add project**, select a local directory, then create **New work** in that project. Select a model, describe your task, and send it.

Work uses **Full access** by default, with no permission selector or first-message confirmation. Tool requests are approved automatically. Commands run with your user account's access on the execution machine.

The first message prepares the bundled runtime automatically. To use Simplified Chinese, open **Settings > General > Language**. Choose **Font size** in the same section for Small, Medium (the default), or Large text; changes apply immediately and are saved on this device.

Paste images with **Ctrl+V** (**Command+V** on macOS), drag files into the composer, or use the attachment button. Remove an attachment with its **×** before sending. Attachments also work in local and SSH project conversations. See [Images and files](docs/user-guide.md#images-and-files) for limits and history.

### 3. Connect an SSH project

Log in once with your system terminal to confirm the host fingerprint and set up a key or ssh-agent. In **Add project > Remote project**, select a saved host from your local SSH config and click **Test connection**. Tandem reuses its username, port, keys, and jump hosts. You can also choose **Enter manually** to enter connection details. Use the folder button (**Browse directories**) beside **Working directory** to select a remote folder, then create the project.

![Tandem remote project dialog with SSH connection details and a working-directory field](docs/images/ssh-project.jpg)

_Choose a remote directory in the project dialog, then start work with the same conversation controls._

**Test connection** only verifies SSH access; it does not upload the server or prepare its runtime. Send the first message in a remote Work conversation to prepare the environment. The message stays pending while Tandem shows preparation progress; first use may take about 10 seconds, or longer depending on the host and connection. Tandem prepares the remote agent over SSH without opening an additional remote network port. Multiple Work conversations on the same host share a background runtime while keeping independent connections and permissions. See the [SSH guide](docs/user-guide.md#ssh-projects) for supported hosts and setup details.

## Run from source

Install Node.js 24, pnpm 12, Rust 1.99.1, just, and the [Tauri development prerequisites](https://v2.tauri.app/start/prerequisites/) for your system.

On macOS, install Xcode Command Line Tools; on Windows, install Visual Studio C++ Build Tools and WebView2. Tests also require Python 3.9 or later and use only its standard library.

```sh
just setup
just dev
```

Tandem uses React, Tauri 2, and Rust services built on GitHub Copilot SDK 1.x from crates.io. `Cargo.lock` currently resolves SDK `1.0.25`, and its matching Copilot CLI `1.0.102` is bundled automatically.

## Learn more

- [User guide](docs/user-guide.md): provider setup, model switching, permissions, context usage, SSH, and telemetry.
- [Development](docs/development.md): checks, browser development, packaging, and releases.
- [Architecture](docs/architecture.md): execution modes, service boundaries, and data flow.
- [Verification](docs/verification.md): completed checks and their limits.
