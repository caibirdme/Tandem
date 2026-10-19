# Original product requirements

Build an agent desktop application similar to Codex desktop. It must connect to remote servers, open remote projects, and start agent sessions there.

The overall interface should closely resemble Codex desktop while offering fewer features. The sidebar has two categories: Chat and Projects. Chat shows conversation history, lets users continue a conversation, and offers a new conversation action.

Projects require an initial setup: connect to a remote server through SSH and select a directory, or select a local directory. Users can then create work sessions within the project, each starting an agent session.

Chat and work have different capabilities. A chat agent cannot access machine directories. It can use tools such as web fetch but must not modify its host environment. Work supports coding tasks: users choose a directory, and the agent interacts with that environment to complete tasks and write code.

Settings lets users configure their own providers and models. Each provider uses exactly one of three protocols: `openai-completions`, `openai-responses`, or `anthropic`. It needs a base URL and API key and may contain multiple models, each with more detailed parameters.

Build agent capabilities on Copilot SDK rather than implementing an agent loop.

## Technology requirements

1. Use Tauri v2 for the desktop, initially supporting macOS and Windows.
2. Write all backend code in Rust, edition 2024 with Rust 1.99.1.
3. Use pnpm, React, TypeScript 7.x, Vite 8.x, and a mature UI component library for the frontend.
4. Pin Copilot SDK to `v1.0.23-preview.5`.
5. Use current Rust dependencies and compatible version ranges in `Cargo.toml` rather than pinning minor or patch releases unnecessarily, so bug fixes can be adopted readily.

## Code quality and documentation

Architecture and code quality are strict requirements.

1. Provide just commands for packaging and lint checks, and run the appropriate checks for each change. Prefix supported commands with `rtk` when useful to reduce output, for example `rtk cargo build`.
2. Enforce oxlint and oxfmt with strong, appropriate checks from the start. Use `cargo clippy` and `cargo +nightly fmt`, with `imports_granularity = "Crate"` and `group_imports = "StdExternalCrate"`. Enable strict Clippy rules and avoid ad hoc `allow` attributes outside test code.
3. Prefer table-driven unit tests. Every test must have a clear purpose; tests are maintained code assets, and readability comes first. Omit tests that offer little value.
4. Maintain project documentation using the `agentsmd-creator` skill. Keep it concise, logically organized, and easy to read. More detail is not inherently better. Follow `how-to-write-tech-doc` for structure and `de-ai-smell` for natural expression.

## Connection and telemetry details

The desktop and remote server communicate through SSH. Once connected, initialize the remote environment automatically, including installing the remote server. Codex desktop offers a reference approach: it starts a remote app server on a Unix socket and then an app-server proxy to bridge the desktop and socket. A similar approach may be used.

Copilot SDK supports OpenTelemetry. Let users configure the export address, protocol, and related settings.

## Delivery and verification

Explore unresolved details independently and consult comparable applications or industry practices. Use Codex desktop as the primary product reference. Deliver a working result without asking the user to resolve routine product choices.

Verify on the current machine that users can configure providers and models, chat, select a local directory, create a project, and run a work session that completes tasks. Also verify that exported OpenTelemetry data is valid.
