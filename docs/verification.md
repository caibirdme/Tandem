# Verification guide

Use this guide to choose checks for the current code. Put each change's actual results in its pull request, not in this file. Git and PR history retain earlier verification evidence.

## Automated checks

Run these from an isolated worktree with the dependencies and toolchains described in [Development](development.md):

Python checks require Python 3.10 or newer and the pinned dependencies in `tests/requirements.txt`. Install them with `python3 -m pip install -r tests/requirements.txt` (`python` on Windows), preferably in a virtual environment; `just setup` includes this step. The telemetry fixtures use gRPC Python and generated OpenTelemetry protobuf messages, not a standard-library-only collector.

```sh
just check
just check-workspace
just build
```

- `just check` covers formatting, frontend lint/types/tests, remote build preparation tests, Rust Clippy/tests, Python tests, and workspace boundaries.
- `just check-workspace` verifies the one-way crate dependencies and keeps desktop, development HTTP, SQLite, and local credential storage out of the remote agent.
- `just build` builds the frontend and both service binaries. It does not verify the native desktop bundle.
- For native UI or resource changes, run `just desktop-build-full`, then open the generated application. Use `just desktop-build` for subsequent builds with the current remote artifact. These commands build a local executable without an installer. For packaging changes, also run `just desktop-bundle` and verify the installer. A successful browser build is not native-desktop verification.
- For remote build preparation changes, run `pnpm test:build`. These tests cover artifact fingerprints, executable targets and hashes, cache reuse, build orchestration, and Docker/Podman selection and failures without requiring a running container engine.

Use focused checks while developing, but do not substitute them for the applicable final gates. Add a regression test for a reproduced bug; where practical, show that it fails before the fix and passes afterward.

## Integration checks

The acceptance suites use the production services and Copilot SDK/CLI with local synthetic inference and telemetry fixtures. They do not require paid provider credentials.

The gRPC collector registers the standard OTLP TraceService and MetricsService Export methods. Its unit tests reject incorrect RPC routes and malformed records; integration checks preserve transient retry and credential-isolation assertions. Loopback plaintext coverage does not establish external TLS interoperability.

| Area changed                            | Command                      | Main coverage                                                                                                       |
| --------------------------------------- | ---------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| Sessions, tools, providers, permissions | `just integration`           | Chat/Work, conversation persistence, tools, cancellation, permissions, and model behavior                           |
| Message staging and steering            | `just integration-steering`  | Busy Chat/Work, immediate delivery at model boundaries, ordinary-send guards, and remote prepared-message transport |
| Provider model discovery                | `just integration-models`    | Catalog pagination, model metadata, authentication, validation, persistence, and credential isolation               |
| AgentLens telemetry                     | `just integration-agentlens` | Local/remote export, content capture, span relationships, retries, credentials, and idle failure recovery           |
| OTLP/gRPC telemetry                     | `just integration-grpc`      | Local/remote traces and metrics, content capture, persistence, credential isolation, and transient retry            |
| Usage dashboard                         | `just integration-usage`     | Actual Chat/Work token usage, skill invocation, model attribution, restart persistence, and remote event forwarding |
| Work change review                      | `just integration-review`    | Real Git baselines, staged/unstaged views, session/turn history, commits, cancellation, ownership, and restart      |
| SSH and remote deployment               | `just integration-ssh`       | Connection, preparation, deployment, permissions, and remote execution                                              |

Build the service binaries first when invoking Python suites directly. The generic runtime suite includes a public web-fetch case and therefore needs network access; SSH acceptance needs local OpenSSH tools and uses a native test server. Keep fixture homes and application data isolated from real user data. Clear inherited provider/offline overrides that could change test behavior; the shared acceptance environment removes inherited trace sampling so expected fixture traces are not randomly omitted.

Synthetic-provider success does not prove compatibility with every real provider or model. A live service check requires explicit authorization. Do not put real keys, private endpoints, user conversations, or other sensitive evidence in commits, PR descriptions, screenshots, or logs.

Review acceptance requires Git 2.45 or newer. The local suite uses real repositories and production SDK tools with synthetic inference. The real SSH suite repeats the review lifecycle over an isolated OpenSSH connection; `python tests/remote_e2e.py --skip-ssh` only verifies remote stdio and, on Unix, the shared daemon. Record which transport was exercised. For capture-performance changes, include a large-file-list fixture and check immutable detail loading after later edits or restart.

## Manual acceptance

Exercise the affected flow in the desktop application with disposable data. At minimum, consider the relevant cases below rather than treating a passing build as user-facing validation:

- **Provider settings:** saved keys remain masked; edit, remove, cancel, and retry failed reads. Discover models with the configured path, confirm none are selected initially, select models, save, and restart.
- **Conversations:** send and stream a reply, switch conversations, scroll long histories, cancel active work, and confirm no stale running indicators. Check persistence after restart and permissions for denied tool actions.
- **Message staging:** while the agent runs, stage several text/skill/attachment drafts without sending them. Edit and restage one, steer another, switch sessions, and verify each item stays with its conversation. Confirm remaining items stay staged on idle, duplicate clicks do not duplicate submissions, failed sends retain their payload, and steering does not cancel a running tool. Check the narrow composer layout and keyboard focus.
- **Remote work:** connection testing and directory browsing must not deploy the agent. An unprepared remote environment initializes on first send with a noninteractive status; a healthy prepared environment is reused by later sessions. A new connection must replace a server whose version or file checksum differs from the desktop artifact before sending the task. Verify that an already connected conversation continues after replacement and that another new conversation reuses the matching artifact without uploading again. `just integration-ssh` covers these cases with isolated SSH and test inference.
- **Detail panel:** open a subagent from its conversation card, follow its output, switch sessions, and close the panel without losing the main conversation state. Resize with pointer and keyboard, toggle the sidebar, cross the narrow-screen drawer breakpoint, and check focus restoration and unsent drafts. DOM tests use fixed geometry and do not replace native pointer or layout checks.
- **Change review:** open Changes in a Work conversation and switch workspace, staged, unstaged, session, and turn scopes. Inspect committed edits and dirty starting files, filter the file list, switch unified/side-by-side views, resize, wrap lines, and close with Escape. Check binary, oversized, incomplete, and unavailable states, deferred details for large file lists, and remote offline cache limits. Confirm Chat has no review action.
- **Usage dashboard:** select 7, 14, and 30 days; inspect provider/model stacks and skill lines with pointer and keyboard. Verify local calendar boundaries, empty/error states, readable K/M labels, and recent session totals. Confirm statistics persist after restart and do not require external telemetry.
- **Telemetry:** verify enabled/disabled and content-capture choices with synthetic data. Test receiver failure and recovery; ordinary snapshots and remote persisted configuration must not expose credentials.
- **Desktop resources:** run `just desktop-build-full`, check that `target/release/remote/x86_64-unknown-linux-gnu/` contains the executable and metadata, and launch the desktop executable to test SSH preparation against Linux x86_64. Confirm Mac and Windows resources contain only this remote target. Run the server with `--version` in a `linux/amd64` Ubuntu 22.04 container and compare its release and protocol with the current checkout. Record the desktop, container, and SSH host platforms actually tested.
- **Packaging:** after preparing the current server, run `just desktop-bundle`, open the generated native installer, and exercise the changed native feature. Record the target platform and installer actually tested; GitHub CI builds the release installers.
- **Build modes:** with a valid artifact, verify that `just remote-check`, `just desktop-build`, and an unchanged `just desktop-build-full` succeed without a running container engine or server compilation. In a disposable checkout, change a remote build input and confirm the ordinary build fails with guidance to use the full build. Start the required engine, run the full build, and confirm it refreshes the artifact and the ordinary build succeeds afterward. A missing or corrupt artifact must also fail validation. Confirm build commands do not start machines or change engine contexts.
- **Container selection:** with a rebuild required, check automatic Docker selection, fallback to Podman when Docker is missing or unreachable, and explicit `TANDEM_CONTAINER_ENGINE=docker` and `TANDEM_CONTAINER_ENGINE=podman` selections. Check invalid override values, a missing CLI, and a stopped or unreachable selected engine; explicit selections must fail with actionable guidance without falling back. On Windows Podman, start the machine manually and verify bind mounts from the actual repository drive. Record which engine and platform completed a real build; mocked command tests do not establish container or filesystem compatibility.

## Evidence in the pull request

Include a concise verification section in every PR:

1. The commands run and their results, tied to the submitted revision.
2. Regression coverage and any pre-fix reproduction.
3. Whether integration used synthetic fixtures or explicitly authorized live services.
4. Manual checks and tested platforms, including relevant UI evidence without private data.
5. Failures, skipped checks, and environment limitations, with the exact blocked stage and any baseline comparison.

Do not describe a blocked command as passing. Linux desktop checks require native GTK/WebKit dependencies; sandbox socket restrictions can block daemon tests. Report these precisely and use platform CI to establish missing coverage rather than suppressing assertions. The path-selected CI jobs and aggregate `Required checks` gate are described in [Development](development.md#pr-checks); the required gate must pass before merging.

Update this guide only when the verification procedure or current coverage changes. Do not append dated change logs, run counts, artifact inventories, or per-PR results.
