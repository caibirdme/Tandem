# Development and delivery

Start every requested change in a separate Git worktree created from `main`, then develop and verify it there. Place worktrees under this repository's `.worktrees/` directory, which is excluded by `.gitignore`:

```sh
git worktree add -b codex/example .worktrees/example main
cd .worktrees/example
```

Use English for project documentation, code comments, commits, PRs, and other project information. Localized UI copy belongs in locale catalogs.

Start the desktop with `just dev`. For UI-only changes, run `just server` and `just web` in separate terminals and open `http://127.0.0.1:1420`. Both modes share the service contract; browser mode uses the built-in directory browser when no native directory picker is available.

`just server` starts `tandem-dev-server`, which calls the same `tandem-core` services as the desktop over HTTP. It builds `tandem-server` first so SSH projects can also be tested during browser development. `tandem-server` provides the remote Unix daemon, its SSH stdio proxy, and an isolated stdio host for protocol verification. It does not host development HTTP.

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
| AgentLens export, content, or credentials     | `just integration-agentlens`                                         |
| Git workspace, session, or turn review        | `just integration-review`, `just integration-ssh`                    |
| SSH transport, deployment, or remote protocol | `just integration-ssh`                                               |
| Windows, resources, or native features        | `just desktop-build`, then open and verify the generated application |

Run `just check` before committing. oxlint enables React, TypeScript, accessibility, and strict rules. TypeScript uses strict mode; Rust runs Clippy all + pedantic with warnings treated as failures. Formatting uses oxfmt and nightly rustfmt; rustfmt merges imports by crate and groups standard library, external crate, and current crate imports.

Unit tests cover state merging, permission denial, configuration boundaries, database relationships, SSH injection, and communication failures. `just check` also runs Python standard-library tests for event routing, process cleanup, and the dependency boundary checker. Integration tests use the real SDK and matching CLI, replacing only external inference and OTLP receivers with test endpoints, so paid credentials are unnecessary. The chat web-tool case reads `https://example.com` and needs public network access. Clear inherited overrides such as `COPILOT_OFFLINE` and `COPILOT_PROVIDER_*` before running, since they can affect isolated test sessions. These tests cannot establish whether a real provider supports a particular model or parameter.

`just integration-review` uses temporary real Git repositories and the production SDK with test inference. It checks dirty initial files, staged cancellation, created/edited/shell-written files, commits during a turn, immutable revisions, session ownership, restart history, non-Git Work, and review RPC without SDK initialization. `just integration-ssh` repeats the Work review lifecycle over its isolated OpenSSH server. CI runs the local/protocol checks on Windows and macOS and the real SSH suite in a separate Ubuntu job included in **Required checks**.

`just integration-agentlens` exercises complete Chat and Work content, excludes real title-helper traffic, checks the AgentLens resource and span conventions, retries a transient collector failure, and verifies local key persistence and remote stdio export with test credentials. CI runs it in the existing Rust job. For an explicitly authorized live receiver check, run `python tests/agentlens_e2e.py --live-endpoint <base-url>` and provide the access key on standard input. This uploads synthetic acceptance conversations using local inference fixtures; it never accepts a key on the command line or prints it. Verify the printed acceptance marker in the intended AgentLens workspace before claiming successful platform display.

## Adding a language

The UI defaults to English. **Settings > General > Language** switches the active locale immediately and stores the choice locally under `tandem.locale`, so it survives application restarts. The language preference is independent of provider configuration and contains no credentials.

**Settings > General > Font size** persists Small, Medium (default), or Large through `save_font_size {fontSize: "small" | "medium" | "large"}`. Snapshot `settings.fontSize` is the confirmed value. This command preserves provider, title, telemetry, and CLI settings and does not stop active sessions. Use rem units for typography so all three sizes apply to new components.

`list_ssh_hosts {}` returns concrete aliases discovered from the local OpenSSH user config and included files. The project picker sends selected aliases as `{kind: "ssh", host: alias, port: null, user: null, identityFile: null}`; OpenSSH resolves connection details without UI-supplied overrides. Manual connections send explicit username and port. Discovery does not evaluate SSH configuration commands.

Keep UI strings in the typed catalogs under `src/i18n/`. To add a language, create a catalog matching the English message keys and parameter signatures, then register its locale code and display name in the locale registry. Use the translation helper in components and the selected locale for dates and counts. Keep protocol identifiers, model IDs, paths, and user content unchanged. Run the frontend checks after adding or changing catalog entries.

## PR checks

When a PR targets `main`, CI runs when the PR is opened, reopened, or updated with new commits. It selects checks from the changed paths and combines their results into `Required checks`. That gate must pass before merging. Relevant pushes to `main` also run CI to maintain trusted baseline caches. If a check fails, open it on the PR page to inspect the failed command's output.

| Check             | Environment                     | Skip condition                                                        | Coverage                                                                                                                       |
| ----------------- | ------------------------------- | --------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `Format`          | Linux                           | Always runs; skips rustfmt when Rust is unchanged                     | oxfmt and nightly rustfmt                                                                                                      |
| `Web`             | macOS, Windows                  | Only Rust, Python verification, or documentation changed              | Frontend tests and production build on both platforms; oxlint and type checks on macOS                                         |
| `Rust`            | macOS, Windows                  | Only frontend or documentation changed                                | Python unit tests, dependency boundaries, Clippy, Rust tests, local SDK integration, remote stdio, and Unix daemon integration |
| `SSH integration` | Linux                           | Same path rules as Rust                                               | Real isolated OpenSSH authentication/deployment, shared daemon, SDK sessions, and Work review lifecycle                        |
| `Desktop bundle`  | Linux container, macOS, Windows | No packaging configuration, icons, dependencies, or toolchain changes | The release packaging procedure, with downloadable artifacts                                                                   |

Unit and local integration checks run on macOS and Windows; a separate Linux job runs the real SSH suite. Linux also handles change detection, formatting, the aggregate gate, and Linux remote binary builds when packaging inputs change. The Rust job builds both services and runs `python tests/remote_e2e.py --skip-ssh` to verify the remote protocol with the real SDK and test inference services. On Unix, this also runs `tests/daemon_acceptance.py`: five independent proxies share one daemon and CLI, with concurrent sessions, separate providers/directories, isolated approvals, prepared-message invalidation, connection-loss cleanup, and explicit history resume. The SSH job installs OpenSSH and runs the same script without `--skip-ssh`, adding actual SSH authentication, deployment, and review lifecycle checks. Run `just integration-ssh` locally when a POSIX SSH daemon is available. The shared helpers in `tests/remote_protocol.py` preserve the same bounded framing and process-cleanup assertions for both modes.

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

`contextWindow` is a required positive integer token count in provider and remote runtime payloads; the Settings editor accepts decimal K units and converts them to tokens at submission. `contextWindow: 1000000` denotes the combined input/output window (enter `1000` K in Settings). When `maxOutputTokens` is explicit, the input budget is the total window minus that output allowance. For example, the two Anthropic models use output `128000`, leaving input `872000`; verification also checks the actual request's `max_tokens`. OpenAI Responses models retain the SDK's default output allowance. Short tasks do not demonstrate compaction behavior near the 1M window; use the [verification guide](verification.md) to select checks and record executed results in the pull request.

## Dependencies and the first build

`rust-toolchain.toml` pins the compiler. Cargo dependencies use compatible major version ranges, or the current minor range for 0.x libraries. The Copilot SDK uses `github-copilot-sdk = "1"` from crates.io; `Cargo.lock` currently resolves `1.0.25`, which bundles Copilot CLI `1.0.102`. Commit `Cargo.lock` and `pnpm-lock.yaml`, and rerun quality gates after upgrades. See the [2026-10-22 dependency inventory](dependency-refresh.md) for version changes and compatibility decisions.

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

`just desktop-build` first compiles `tandem-server` for the local platform and bundles the desktop application. On supported remote execution platforms, it copies that server into Tauri resources. Windows desktop builds require separately built Linux or macOS remote artifacts to prepare SSH projects; the release workflow supplies both. `tandem-dev-server` is excluded from installers. macOS output is in `target/release/bundle/`. Local signing is ad hoc by default. For a formally signed release, override `bundle.macOS.signingIdentity` and configure notarization. Signing and notarization require a release account; no credentials are stored in this repository.

On Windows, use the NSIS `x64-setup.exe` installer in `target/release/bundle/nsis/`. Tauri automatically merges `src-tauri/tauri.windows.conf.json`, which selects NSIS as the only Windows installer target.

Use `CI=true just desktop-build` on a locked screen or in automation to skip DMG window decoration that requires Finder interaction.

Released installers bundle remote agents for Linux x64 (`x86_64-unknown-linux-gnu`) and Apple Silicon macOS (`aarch64-apple-darwin`). The application also recognizes Linux arm64 and Intel Mac hosts, but installers do not contain those binaries and report a missing artifact when preparing the first remote message. Build the required target yourself and use `scripts/prepare-remote.mjs <target>` to add it to resources.

The Windows executable without an installer is `target/release/tandem-desktop.exe`; the remote artifact is `tandem-server`. Set `TANDEM_REMOTE_ARTIFACTS` for a custom artifact directory containing that binary.

SSH uses BatchMode and strict host verification, supporting keys and ssh-agent. For an unknown host or interactive login, log in from a terminal first. Windows clients require system OpenSSH; Windows remote execution hosts are not supported.

## Releases

1. Set the new version in `package.json`, `src-tauri/tauri.conf.json`, and `Cargo.toml`'s `workspace.package`. Run `cargo update --workspace` to synchronize `Cargo.lock`, then merge through a PR into `main`.
2. Tag `main` and push the tag:

   ```sh
   git tag v0.0.3
   git push origin v0.0.3
   ```

3. Wait for the Release workflow in Actions, open the matching draft on the Releases page, review its installers, and publish it.

The commands above use `0.0.3` as an example; use the version merged in step 1. The tag must be `v` followed by that version. Otherwise, `Check version` lists inconsistent files and stops before packaging. Publish the Release only after the workflow has attached the installers.

If version validation fails, correct all three manifests and synchronize `Cargo.lock` before merging. Rerunning the failed workflow uses its original commit, so a fix on `main` alone does not repair an existing tag. Use a new matching version and tag, or deliberately correct the existing tag and Release after reviewing their published state.

The workflow builds Linux x64 `tandem-server` in an `ubuntu:22.04` container for compatibility with older glibc, and builds the Apple Silicon agent on macOS. Desktop jobs place both agents in resources and generate an Apple Silicon DMG plus a Windows x64 NSIS `x64-setup.exe` installer. The development host is excluded.

macOS uses ad hoc signing without an Apple account. Users must remove quarantine before first launch; see [Install](../README.md#install). To obtain installers without publishing a release, manually run **Desktop bundles** in Actions and download its artifacts. Rerunning a tag workflow replaces files with the same names in its Release.

## Service contract

The desktop calls `dispatch`; development mode calls `POST /api/dispatch`. Requests are `{ command, payload }`. Types are centralized in `src/lib/types.ts` and Rust's `Request`; update both sides together. Streaming events use Tauri's `runtime-event` or `/api/events`.

Review uses `get_review_summary` or `refresh_review` with `{sessionId, selection}`. Selection is `{kind: "workspace", comparison: "net" | "staged" | "unstaged"}`, `{kind: "session"}`, or `{kind: "turn", messageId}`. Responses contain a nullable summary, a separate unavailable reason, and turn navigation metadata. `list_review_files {sessionId, reviewId, revision, cursor?}` returns a bounded page; `get_review_file {sessionId, reviewId, revision, fileId}` returns a bounded patch and structured hunks or an explicit non-text state. The same immutable revision must be used throughout. `review_updated`/`review_invalidated` are registered in the event transport. Remote protocol version 3 adds connection-scoped workspace inspection commands without adding core, HTTP, or database dependencies to the remote executable.

Title settings use the separate command `save_title_model {titleModel: {providerId, modelId} | null}`. `null` follows the current session model, and `save_preferences` preserves the setting. `regenerate_title {sessionId}` only queues a background task. Completion sends a `title {title}` event; failure sends `title_error {message}`. Neither changes session execution state. Snapshot `titleGenerations` restores generation progress.

The provider editor reads the saved key with `get_provider_api_key` (`id`), which returns a string or `null` and errors if the provider does not exist. Ordinary `snapshot` and `save_provider` responses contain only `hasApiKey`. Omitting `apiKey` from a save preserves the value; a nonempty string replaces it; an empty string clears it. Keys are stored only in local SQLite's `provider_secrets.secret`, never in messages, logs, or remote state. The development host's `--data-dir` selects storage for both configuration and keys; `--ephemeral-secrets` switches to an in-process test store.

`just integration-models` verifies discovery against local HTTP fixtures and the real development host without paid credentials. CI runs this check before SDK integration.

`list_provider_models {baseUrl, listModelsPath, apiKey?}` explicitly queries the current provider draft without saving it or reading saved credentials. It returns model IDs and display names, with optional `contextWindow`, `maxOutputTokens`, and `supportedReasoningEfforts`, and does not choose models. Catalog `max_input_tokens` maps directly to `contextWindow`; `max_tokens` maps to `maxOutputTokens`. Missing or null token limits remain unspecified. Catalog values are independent advertised ceilings: each uses the same numeric bounds as provider configuration, but an output ceiling equal to or larger than the input ceiling does not invalidate discovery. Saving an actual provider allocation still requires output below context so there is room for input. `capabilities.effort` contributes only supported levels in `low`, `medium`, `high`, `xhigh`, `max` order. Missing effort metadata leaves the supported set unspecified; an explicit unsupported capability produces an empty set. Adding selected results fills these fields and saves the supported set with the provider model. `listModelsPath` is stored with the provider; omitted values deserialize to `/v1/models`. The backend uses the [official Anthropic Models API](https://platform.claude.com/docs/en/api/models/list?version=2026-10-21): `x-api-key`, `anthropic-version: 2023-06-01`, and `after_id` pagination from `last_id` while `has_more` is true. The documentation URL's version query is not an API header version. No provider-specific alternate discovery protocol is inferred.

Discovery appends a relative path to the validated base URL, retaining gateway prefixes. Base URLs continue to reject queries and fragments; a custom list path can include non-pagination query parameters. Discovery owns `limit`, `after_id`, and `before_id` parameters. It rejects absolute/network-relative paths and traversal, disables redirects, and bounds time, page bytes, pages, and model count. Errors exclude upstream response content and request URLs. Model IDs are deduplicated in catalog order. The UI clears results when connection details change and discards stale request completions.

SSH `check_connection` verifies connectivity only; creating a project/session or scanning skills does not deploy the server. On the first remote send, `preparation {preparing: true}` begins the pending environment stage; `preparation {preparing: false}` ends it before message confirmation. Snapshots include `preparingSessions` for reconnect recovery. `abort_session` can cancel this stage before the user message is persisted or sent.

`send_message` requires a caller-supplied `messageId` UUID. The saved user message retains that ID so the UI can merge it accurately with history after a lost live confirmation. A retry with an already persisted message ID is rejected as a conflict without executing it again; the caller should review the recorded conversation instead of assuming delivery. Overlapping submissions are rejected while the session is busy.

User messages also retain their validated `selectedSkills` identities. Both the send confirmation and history return these selections, allowing the UI to distinguish selected skill mentions from ordinary `$` text. Empty selections are omitted.

Thinking uses ordinary message updates with `role: "reasoning"`, text in `content`, and `reasoning: {status}`. SDK `assistant.reasoning_delta` and `assistant.reasoning` events supply the displayed text; opaque reasoning fields are excluded. Streamed updates keep a stable message ID and sequence. The core checkpoints the first chunk and lifecycle transitions, and overlays current in-memory text when reading history. The frontend shows a collapsed header and a three-line scroll area.

`send_message` also accepts `attachments: [{name, mimeType, data}]`, where `data` is raw base64 without a data-URL prefix. Text can be empty when attachments are present. Rust validates at most 10 attachments, 10 MiB per file, and 20 MiB in total before initializing a turn. Supported content is PNG/JPEG/GIF/WebP, UTF-8 text, and PDF on Responses/Anthropic. New PDF sends, model changes, and provider edits reject incompatible Completions transport before changing history, credentials, or settings. The persisted `Message.attachments` contains `{id, name, mimeType, size}` metadata only. `get_attachment {sessionId, attachmentId}` returns the original `{name, mimeType, data}` for a matching conversation. Attachment rows are committed with the message and cascade on deletion. The HTTP body and SSH frame limits are 32 MiB.

The desktop window uses `dragDropEnabled: false` so HTML file drops reach the composer on Windows. Clipboard images and file selection use browser `File` objects; the app does not expose a general path-reading command. Attachment previews use the existing `data:` image policy. Attachment acceptance in the local and remote suites checks the actual SDK provider requests, persisted retrieval, and resumed context using test inference services.

For work, `create_session` accepts `permissionMode: "ask" | "allow-all"`, defaulting to `allow-all` in the session service. Chat defaults to `ask` and cannot use the Work full-access setting. The composer always requests full access for Work and has no permission selector or first-message reminder. Inactive work can switch through `set_permission_mode` (`sessionId`, `permissionMode`), which returns the updated session. Switching is rejected during execution, initialization, or pending approval. Both SDK integration suites cover automatically approved real file/command operations, denial after explicitly switching back to ask mode, and recovery with a new mode. Local verification also checks persistence after restarting application services.

Existing chat or work switches models with `set_session_model {sessionId, providerId, modelId}`, returning the updated session. A model from any configured provider can be selected when its protocol supports the conversation's attachments; session ID, history, title, project, and tool permissions are preserved. Switching is rejected during execution, initialization, or pending approval. The selection and provider foreign key are persisted together; the old agent connection is invalidated and disconnected. The next message resumes the original SDK session with the new provider, model, and parameters. Old connection cleanup completes even if the switch request is canceled. While the result is unconfirmed, the UI blocks sending and lets users reread the actual model selection.

Remote transport uses newline-delimited JSON defined by `tandem-protocol`: requests `{id,command,payload}`, responses `{id,value}` or `{id,error:{code,message}}`, and asynchronous events `{event}`. This SSH client/remote agent protocol is maintained separately from the development host's HTTP command entry point. stdout is reserved for the protocol; diagnostics go to stderr. Never log request contents or provider credentials.

For both chat and work, `create_session` accepts optional `reasoningEffort: "low" | "medium" | "high" | "xhigh" | "max" | null`. `set_session_reasoning_effort {sessionId, reasoningEffort}` requires that field and returns the updated session. `null` follows the selected provider model's default; an explicit value overrides it for this conversation. Provider model `supportedReasoningEfforts` is optional: an unspecified set permits all five presets, while an empty set permits only `null`. Provider defaults and session selections are validated against an explicitly supplied set. Model changes preserve supported overrides and reset unsupported overrides to `null`; provider edits narrowing support also clear incompatible session overrides. Effort values are displayed and passed to the SDK unchanged, including in translated interfaces. The override is persisted independently of provider settings. Updates are accepted during execution and pending approval and take effect on the next message. Before sending, the core reconnects the existing SDK session only when the resolved effort differs from its live configuration. The active response, conversation history, title, context reading, and permissions are preserved. SSH uses the resolved `SessionOptions.model.reasoningEffort` without adding an RPC command. Invalid efforts return `invalid_input`; a missing session returns `not_found`.

## CI cache baselines and timing

CI runs the same path-filtered checks on pull requests and non-documentation pushes to `main`. Main push jobs populate trusted cache baselines that later PRs can restore. PR merge-ref caches are isolated from other PRs, so PR jobs only restore caches rather than uploading another large copy after every new branch. The first PR with new cache keys is expected to be cold; the baseline becomes available after a successful main push.

- Cargo caches include dependency build outputs and the SDK's checksum-verified download archives. Keys distinguish debug checks, each remote release target (including the Linux container ABI), and each desktop release platform. The cache action also hashes the compiler, Cargo manifests/lockfile, and relevant Rust environment. Workspace crates and installed Cargo tools are not cached. CI debug/test jobs omit debug symbols to reduce cache size; debug assertions, tests, local development, and release profiles are unchanged.
- `BUNDLED_CLI_CACHE_DIR` lets the SDK reuse verified archives across different Cargo build variants in the same job. Its contents are included in the corresponding Rust cache, not application credentials or runtime session data.
- Node jobs restore the pnpm content-addressed store keyed by OS, architecture, lockfile, and package-manager manifest. They still run `pnpm install --frozen-lockfile`; `node_modules` is not cached. Only main push runs save a missing store baseline.
- Release tags and manually dispatched bundles may restore the baseline but do not write it. No privileged PR trigger or write token is used. Never cache credentials, user data, or artifacts downloaded from untrusted builds.
- All existing platform checks, integration suites, bundle conditions, and the `Required checks` gate remain in place. New pushes cancel older runs for the same PR or branch, not unrelated PRs.

To evaluate performance, compare a cold PR, the first successful main baseline build, and a subsequent PR with unchanged dependencies. Record each Rust cache step's exact-match summary, restore/save duration and archive size, Clippy/test/build durations, and the end-to-end critical path. A cache miss or a key change is not a correctness failure. Expect a lower hit rate after dependency/compiler upgrades; do not claim a fixed speedup before observing warm runs. Main builds add CI work in exchange for reuse across future PRs. Monitor repository cache usage before expanding the cached paths or enabling workspace-crate caching.
