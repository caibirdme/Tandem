# Tandem

An agent desktop for chat, local projects, and SSH work. Bring your own model provider and API key to ask questions, explore a codebase, edit files, and run project tasks.

No GitHub Copilot subscription is required. Available for Windows and Apple Silicon Mac.

[Download](https://github.com/caibirdme/Tandem/releases) · [Get started](#get-started) · [User guide](docs/user-guide.md) · [Development](docs/development.md)

![A Tandem work session showing a project conversation, model selection, and permission controls](docs/images/work-session.jpg)

_Keep project conversations, model selection, and permission controls together in one workspace. Screenshots use illustrative data._

## What you can do

- **Chat with your models.** Connect OpenAI-compatible APIs, Anthropic, or your own model gateway. Chat can read web pages.
- **Include images and files.** Paste an image or drop files into the composer, review the previews, and send them with your message.
- **Work where your files are.** Open a local directory or connect to a Linux or macOS host over SSH. The agent can read and edit files and run commands.
- **Choose how the agent works.** Approve each tool permission request or grant full access for a work session. Switch models and choose reasoning effort for each conversation.

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

Work starts with **Ask each time**. Use the control beside the composer to choose **Full access** if you want tool requests approved automatically. Commands run with your user account's access on the execution machine.

The first message prepares the bundled runtime automatically. To use Simplified Chinese, open **Settings > General > Language**.

Paste images with **Ctrl+V** (**Command+V** on macOS), drag files into the composer, or use the attachment button. Remove an attachment with its **×** before sending. Attachments also work in local and SSH project conversations. See [Images and files](docs/user-guide.md#images-and-files) for limits and history.

### 3. Connect an SSH project

Log in once with your system terminal to confirm the host fingerprint and set up a key or ssh-agent. In **Add project**, choose **Remote project**, enter the SSH connection details, and click **Test connection**. Use the folder button (**Browse directories**) beside **Working directory** to select a remote folder, then create the project.

![Tandem remote project dialog with SSH connection details and a working-directory field](docs/images/ssh-project.jpg)

_Choose a remote directory in the project dialog, then start work with the same conversation and permission controls._

Tandem prepares the remote agent over SSH without opening an additional remote network port. Multiple Work conversations on the same host share a background runtime while keeping independent connections and permissions. See the [SSH guide](docs/user-guide.md#ssh-projects) for supported hosts and setup details.

## Run from source

Install Node.js 24, pnpm 12, Rust 1.99.1, just, and the [Tauri development prerequisites](https://v2.tauri.app/start/prerequisites/) for your system.

On macOS, install Xcode Command Line Tools; on Windows, install Visual Studio C++ Build Tools and WebView2. Tests also require Python 3.9 or later and use only its standard library.

```sh
just setup
just dev
```

Tandem uses React, Tauri 2, and Rust services built on GitHub Copilot SDK 1.x from crates.io. `Cargo.lock` currently resolves SDK `1.0.24`, and its matching Copilot CLI `1.0.101` is bundled automatically.

## Learn more

- [User guide](docs/user-guide.md): provider setup, model switching, permissions, context usage, SSH, and telemetry.
- [Development](docs/development.md): checks, browser development, packaging, and releases.
- [Architecture](docs/architecture.md): execution modes, service boundaries, and data flow.
- [Verification](docs/verification.md): completed checks and their limits.
