# Development and delivery

Start every requested change in a separate Git worktree created from `main`, then develop and verify it there. Place worktrees under this repository's `.worktrees/` directory, which is excluded by `.gitignore`:

```sh
git worktree add -b codex/example .worktrees/example main
cd .worktrees/example
```

Use English for project documentation, code comments, commits, PRs, and other project information. Localized UI copy belongs in locale catalogs.

Start the desktop with `just dev`. For UI-only changes, run `just server` and `just web` in separate terminals and open `http://127.0.0.1:1420`. Both modes share the service contract; browser mode uses the built-in directory browser when no native directory picker is available.

`just server` starts `tandem-dev-server`, which calls the same `tandem-core` services as the desktop over HTTP. It builds `tandem-server` first so SSH projects can also be tested during browser development. `tandem-server` itself provides only the remote agent stdio entry point; it no longer hosts development HTTP.

The workspace default target is also `tandem-dev-server`, so `cargo run` starts the local HTTP host. Select `-p tandem-server` explicitly when building the remote agent.

## Checks for each change

| Change                                        | Checks                                                               |
| --------------------------------------------- | -------------------------------------------------------------------- |
| React / TypeScript                            | `just lint-web`, `pnpm test`, `pnpm build`                           |
| Rust services or SDK integration              | `just lint-rust`, `cargo test --workspace`                           |
| Crate dependencies and service entry points   | `just check-workspace`                                               |
| Verification scripts and event waiting        | `just test-python`                                                   |
| Formatting                                    | `just format`, then `just check-format`                              |
| Sessions, tools, providers, or telemetry      | `just integration`                                                   |
| SSH transport, deployment, or remote protocol | `just integration-ssh`                                               |
| Windows, resources, or native features        | `just desktop-build`, then open and verify the generated application |

Run `just check` before committing. oxlint enables React, TypeScript, accessibility, and strict rules. TypeScript uses strict mode; Rust runs Clippy all + pedantic with warnings treated as failures. Formatting uses oxfmt and nightly rustfmt; rustfmt merges imports by crate and groups standard library, external crate, and current crate imports.

Unit tests cover state merging, permission denial, configuration boundaries, database relationships, SSH injection, and communication failures. `just check` also runs Python standard-library tests for event routing, process cleanup, and the dependency boundary checker. Integration tests use the real SDK and matching CLI, replacing only external inference and OTLP receivers with test endpoints, so paid credentials are unnecessary. The chat web-tool case reads `https://example.com` and needs public network access. Clear inherited overrides such as `COPILOT_OFFLINE` and `COPILOT_PROVIDER_*` before running, since they can affect isolated test sessions. These tests cannot establish whether a real provider supports a particular model or parameter.

## Adding a language

The UI defaults to English. **Settings > General > Language** switches the active locale immediately and stores the choice locally under `tandem.locale`, so it survives application restarts. The language preference is independent of provider configuration and contains no credentials.

Keep UI strings in the typed catalogs under `src/i18n/`. To add a language, create a catalog matching the English message keys and parameter signatures, then register its locale code and display name in the locale registry. Use the translation helper in components and the selected locale for dates and counts. Keep protocol identifiers, model IDs, paths, and user content unchanged. Run the frontend checks after adding or changing catalog entries.

## PR checks

When a PR targets `main`, CI runs when the PR is opened, reopened, or updated with new commits. It selects checks from the changed paths and combines their results into `Required checks`. That gate must pass before merging. Merging a PR or pushing directly to `main` does not start CI. If a check fails, open it on the PR page to inspect the failed command's output.

| Check            | Environment                     | Skip condition                                                        | Coverage                                                                                                          |
| ---------------- | ------------------------------- | --------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| `Format`         | Linux                           | Always runs; skips rustfmt when Rust is unchanged                     | oxfmt and nightly rustfmt                                                                                         |
| `Web`            | macOS, Windows                  | Only Rust, Python verification, or documentation changed              | Frontend tests and production build on both platforms; oxlint and type checks on macOS                            |
| `Rust`           | macOS, Windows                  | Only frontend or documentation changed                                | Python unit tests, dependency boundaries, Clippy, Rust tests, local SDK integration, and remote stdio integration |
| `Desktop bundle` | Linux container, macOS, Windows | No packaging configuration, icons, dependencies, or toolchain changes | The release packaging procedure, with downloadable artifacts                                                      |

PR tests run only on macOS and Windows. Linux still handles change detection, formatting, the aggregate gate, and Linux remote binary builds when packaging inputs change. The Rust job builds both services and runs `python tests/remote_e2e.py --skip-ssh` to verify the remote protocol with the real SDK and test inference services. Run `just integration-ssh` manually when changing SSH transport, deployment, or the remote protocol to verify actual SSH authentication and deployment as well.

Skipped checks count as successful. The `changes` job in `.github/workflows/ci.yml` defines the path rules. Format, web, and Rust checks default to relevant changes unless all modified paths match their exclusions. New directories therefore trigger the relevant checks automatically. Packaging runs only when listed inputs change. Update these rules when adding build inputs or check commands.

Rust build caches are saved within the PR's scope and reused by later updates to the same PR. GitHub also allows PRs to restore eligible caches from their base branch. New commits cancel any earlier CI run for the same PR.

A repository rule enforces the merge gate and requires one-time administrator setup. In GitHub **Settings > Rules > Rulesets**, create a branch rule for the default branch, enable **Require a pull request before merging** and **Require status checks to pass**, and add **Required checks** from GitHub Actions.

## Verification with real providers

`tests/live_provider_e2e.py` uses real inference requests to check chat, streaming message persistence, and work that reads, creates, and edits files and executes a verification command. It connects to `https://camp-dev.woa.com/pingress`, so it requires access to that gateway and a valid provider key. It incurs real inference usage and is excluded from routine quality gates.

Run from the repository root:

```sh
# Build the local development host; the script starts and stops it automatically.
cargo build -p tandem-dev-server

# Enter the key at the Provider API key prompt; terminal input is hidden.
python3 tests/live_provider_e2e.py --output artifacts/live-provider-report.json
```

By default, the script tests four models sequentially, running chat and work cases for each. `claude-haiku-5.5` and `glm-5.3-flashx` use Anthropic; `gpt-6.1-sol` and `deepseek-v4.1-flash` use OpenAI Responses. A local observation proxy forwards request bodies unchanged to the HTTPS gateway without modifying model or token parameters.

Interactive terminals hide key input. Noninteractive calls read one line from standard input; callers must supply it through a protected input channel, never command arguments, environment variables, or script files. Real-provider verification explicitly uses temporary application data and an in-memory credential store. It neither changes desktop provider configuration nor writes real keys to disk. SDK integration with test keys covers default local persistence and restart recovery. Work approvals allow only file access inside the temporary project, writes to designated result files, and a pre-generated verification command.

To retry one or more models, list their IDs with `--models` and keep a separate report for that run:

```sh
python3 tests/live_provider_e2e.py --models deepseek-v4.1-flash --output artifacts/live-provider-deepseek-retry.json

# Check only the verification script's policies; no key or inference request is needed.
python3 tests/live_provider_e2e.py --self-test
```

Each retry creates new isolated sessions and reruns chat and work for the selected models. `--output` updates the redacted JSON report after each case. The final report contains case outcomes, request counts, HTTP statuses, timing, actual output token parameters, and secret-scan results for temporary application data and logs. Request observation records neither bodies nor authorization headers, and keys are replaced in error text. Exit code `0` means all selected cases and checks passed; a single-model retry establishes results only for that model.

`contextWindow` is a required positive integer token count in provider and remote runtime payloads; the Settings editor accepts decimal K units and converts them to tokens at submission. `contextWindow: 1000000` denotes the combined input/output window (enter `1000` K in Settings). When `maxOutputTokens` is explicit, the input budget is the total window minus that output allowance. For example, the two Anthropic models use output `128000`, leaving input `872000`; verification also checks the actual request's `max_tokens`. OpenAI Responses models retain the SDK's default output allowance. Short tasks do not demonstrate compaction behavior near the 1M window; see the [verification record](verification.md) for executed results.

## Dependencies and the first build

`rust-toolchain.toml` pins the compiler. Cargo dependencies use compatible major version ranges, or the current minor range for 0.x libraries. The SDK is an exception and uses the required Git tag. Commit `Cargo.lock` and `pnpm-lock.yaml`, and rerun quality gates after upgrades.

The SDK's first build downloads the platform CLI; prepare a build cache before working offline. If Cargo reports mirror TLS or DNS failures, inspect local Cargo source configuration first. Do not weaken certificate verification.

Service build targets are listed below. Use the development host for local application debugging and build only the remote agent for remote distribution.

| Command                | Build target and purpose                                                                          |
| ---------------------- | ------------------------------------------------------------------------------------------------- |
| `just build`           | Frontend plus debug `tandem-dev-server` and `tandem-server`                                       |
| `just build-server`    | Both debug service binaries for local and SSH development                                         |
| `just integration`     | Builds only `tandem-dev-server` and runs local SDK verification                                   |
| `just integration-ssh` | Builds both services, starts the local development host, and deploys the remote agent through SSH |
| `just remote-build`    | Builds only release `tandem-server` and copies it into desktop resources                          |

Shared RPC definitions live in `tandem-protocol`, so the remote agent does not need the entire SSH client or application service merely to read requests. When changing dependencies, check the [architecture boundaries](architecture.md#responsibilities-and-dependencies). Keep development HTTP, SQLite, and system credential stores out of the remote binary.

`just check-workspace` calls `scripts/check_workspace.py`. It reads Cargo metadata's normal and build dependencies, verifies one-way relationships among the seven workspace crates, and inspects `tandem-server`'s dependency closure. Local application services, the SSH client, the development host, Axum, SQLite, system credential stores, and Tauri are rejected from the remote agent. This check runs in `just check` and CI. Test-only dependencies are excluded from the distribution dependency calculation.

## Packaging and remote artifacts

`just desktop-build` first compiles `tandem-server` for the local platform, copies it into Tauri resources, and bundles the desktop application. `tandem-dev-server` is excluded from installers. macOS output is in `target/release/bundle/`. Local signing is ad hoc by default. For a formally signed release, override `bundle.macOS.signingIdentity` and configure notarization. Signing and notarization require a release account; no credentials are stored in this repository.

Use `CI=true just desktop-build` on a locked screen or in automation to skip DMG window decoration that requires Finder interaction.

Released installers bundle remote agents for Linux x64 (`x86_64-unknown-linux-gnu`) and Apple Silicon macOS (`aarch64-apple-darwin`). The application also recognizes Linux arm64 and Intel Mac hosts, but installers do not contain those binaries and report a missing artifact when connecting. Build the required target yourself and use `scripts/prepare-remote.mjs <target>` to add it to resources.

The Windows executable without an installer is `target/release/tandem-desktop.exe`; the remote artifact is `tandem-server`. Set `TANDEM_REMOTE_ARTIFACTS` for a custom artifact directory containing that binary.

SSH uses BatchMode and strict host verification, supporting keys and ssh-agent. For an unknown host or interactive login, log in from a terminal first. Windows clients require system OpenSSH; Windows remote execution hosts are not supported.

## Releases

1. Set the new version in `package.json`, `src-tauri/tauri.conf.json`, and `Cargo.toml`'s `workspace.package`. Run `cargo update --workspace` to synchronize `Cargo.lock`, then merge through a PR into `main`.
2. Tag `main` and push the tag:

   ```sh
   git tag v0.0.2
   git push origin v0.0.2
   ```

3. Wait for the Release workflow in Actions, open the matching draft on the Releases page, review its installers, and publish it.

The commands above use `0.0.2` as an example; use the version merged in step 1. The tag must be `v` followed by that version. Otherwise, `Check version` lists inconsistent files and stops before packaging. Publish the Release only after the workflow has attached the installers.

If version validation fails, correct all three manifests and synchronize `Cargo.lock` before merging. Rerunning the failed workflow uses its original commit, so a fix on `main` alone does not repair an existing tag. Use a new matching version and tag, or deliberately correct the existing tag and Release after reviewing their published state.

The workflow builds Linux x64 `tandem-server` in an `ubuntu:22.04` container for compatibility with older glibc, and builds the Apple Silicon agent on macOS. Desktop jobs place both agents in resources and generate an Apple Silicon DMG plus Windows x64 MSI and NSIS installers. The development host is excluded.

macOS uses ad hoc signing without an Apple account. Users must remove quarantine before first launch; see [Install](../README.md#install). To obtain installers without publishing a release, manually run **Desktop bundles** in Actions and download its artifacts. Rerunning a tag workflow replaces files with the same names in its Release.

## Service contract

The desktop calls `dispatch`; development mode calls `POST /api/dispatch`. Requests are `{ command, payload }`. Types are centralized in `src/lib/types.ts` and Rust's `Request`; update both sides together. Streaming events use Tauri's `runtime-event` or `/api/events`.

Title settings use the separate command `save_title_model {titleModel: {providerId, modelId} | null}`. `null` follows the current session model, and `save_preferences` preserves the setting. `regenerate_title {sessionId}` only queues a background task. Completion sends a `title {title}` event; failure sends `title_error {message}`. Neither changes session execution state. Snapshot `titleGenerations` restores generation progress.

The provider editor reads the saved key with `get_provider_api_key` (`id`), which returns a string or `null` and errors if the provider does not exist. Ordinary `snapshot` and `save_provider` responses contain only `hasApiKey`. Omitting `apiKey` from a save preserves the value; a nonempty string replaces it; an empty string clears it. Keys are stored only in local SQLite's `provider_secrets.secret`, never in messages, logs, or remote state. The development host's `--data-dir` selects storage for both configuration and keys; `--ephemeral-secrets` switches to an in-process test store.

`send_message` requires a caller-supplied `messageId` UUID. The saved user message retains that ID so the UI can merge it accurately with history after a lost live confirmation. The database rejects repeated submissions with the same ID rather than executing them again.

For work, `create_session` accepts `permissionMode: "ask" | "allow-all"`, defaulting to `ask`. Inactive work can switch through `set_permission_mode` (`sessionId`, `permissionMode`), which returns the updated session. Chat cannot use this full-access setting. Switching is rejected during execution, initialization, or pending approval. Both SDK integration suites cover automatically approved real file/command operations, denial after switching back to ask mode, and recovery with a new mode. Local verification also checks persistence after restarting application services.

Existing chat or work switches models with `set_session_model {sessionId, providerId, modelId}`, returning the updated session. Any model from any configured provider can be selected; session ID, history, title, project, and tool permissions are preserved. Switching is rejected during execution, initialization, or pending approval. The selection and provider foreign key are persisted together; the old agent connection is invalidated and disconnected. The next message resumes the original SDK session with the new provider, model, and parameters. Old connection cleanup completes even if the switch request is canceled. While the result is unconfirmed, the UI blocks sending and lets users reread the actual model selection.

Remote transport uses newline-delimited JSON defined by `tandem-protocol`: requests `{id,command,payload}`, responses `{id,value}` or `{id,error:{code,message}}`, and asynchronous events `{event}`. This SSH client/remote agent protocol is maintained separately from the development host's HTTP command entry point. stdout is reserved for the protocol; diagnostics go to stderr. Never log request contents or provider credentials.

For both chat and work, `create_session` accepts optional `reasoningEffort: "low" | "medium" | "high" | null`. `set_session_reasoning_effort {sessionId, reasoningEffort}` requires that field and returns the updated session. `null` follows the selected provider model's default; an explicit value overrides it for this conversation, including after a model change. The override is persisted independently of provider settings. Updates are accepted during execution and pending approval and take effect on the next message. Before sending, the core reconnects the existing SDK session only when the resolved effort differs from its live configuration. The active response, conversation history, title, context reading, and permissions are preserved. SSH uses the resolved `SessionOptions.model.reasoningEffort` without adding an RPC command. Invalid efforts return `invalid_input`; a missing session returns `not_found`.
