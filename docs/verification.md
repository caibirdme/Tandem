# Verification guide

Use this guide to choose checks for the current code. Put each change's actual results in its pull request, not in this file. Git and PR history retain earlier verification evidence.

## Automated checks

Run these from an isolated worktree with the dependencies and toolchains described in [Development](development.md):

```sh
just check
just check-workspace
just build
```

- `just check` covers formatting, frontend lint/types/tests, Rust Clippy/tests, Python tests, and workspace boundaries.
- `just check-workspace` verifies the one-way crate dependencies and keeps desktop, development HTTP, SQLite, and local credential storage out of the remote agent.
- `just build` builds the frontend and both service binaries. It does not verify the native desktop bundle.
- For native UI, resources, or packaging changes, run `just desktop-build`, then open the generated application. A successful browser build is not native-desktop verification.

Use focused checks while developing, but do not substitute them for the applicable final gates. Add a regression test for a reproduced bug; where practical, show that it fails before the fix and passes afterward.

## Integration checks

The acceptance suites use the production services and Copilot SDK/CLI with local synthetic inference and telemetry fixtures. They do not require paid provider credentials.

| Area changed                            | Command                      | Main coverage                                                                                             |
| --------------------------------------- | ---------------------------- | --------------------------------------------------------------------------------------------------------- |
| Sessions, tools, providers, permissions | `just integration`           | Chat/Work, conversation persistence, tools, cancellation, permissions, and model behavior                 |
| Provider model discovery                | `just integration-models`    | Catalog pagination, model metadata, authentication, validation, persistence, and credential isolation     |
| AgentLens telemetry                     | `just integration-agentlens` | Local/remote export, content capture, span relationships, retries, credentials, and idle failure recovery |
| OTLP/gRPC telemetry                     | `just integration-grpc`      | Local/remote traces and metrics, content capture, persistence, credential isolation, and transient retry  |
| SSH and remote deployment               | `just integration-ssh`       | Connection, preparation, deployment, permissions, and remote execution                                    |

Build the service binaries first when invoking Python suites directly. The generic runtime suite includes a public web-fetch case and therefore needs network access; SSH acceptance needs local OpenSSH tools. Keep fixture homes and application data isolated from real user data. Clear inherited provider/offline overrides that could change test behavior; the shared acceptance environment removes inherited trace sampling so expected fixture traces are not randomly omitted.

Synthetic-provider success does not prove compatibility with every real provider or model. A live service check requires explicit authorization. Do not put real keys, private endpoints, user conversations, or other sensitive evidence in commits, PR descriptions, screenshots, or logs.

## Manual acceptance

Exercise the affected flow in the desktop application with disposable data. At minimum, consider the relevant cases below rather than treating a passing build as user-facing validation:

- **Provider settings:** saved keys remain masked; edit, remove, cancel, and retry failed reads. Discover models with the configured path, confirm none are selected initially, select models, save, and restart.
- **Conversations:** send and stream a reply, switch conversations, scroll long histories, cancel active work, and confirm no stale running indicators. Check persistence after restart and permissions for denied tool actions.
- **Remote work:** connection testing and directory browsing must not deploy the agent. An unprepared remote environment initializes on first send with a noninteractive status; a healthy prepared environment is reused by later sessions.
- **Detail panel:** open a subagent from its conversation card, follow its output, switch sessions, and close the panel without losing the main conversation state.
- **Telemetry:** verify enabled/disabled and content-capture choices with synthetic data. Test receiver failure and recovery; ordinary snapshots and remote persisted configuration must not expose credentials.
- **Packaging:** launch the generated native bundle on the target platform and exercise the changed native feature. Record which platforms were actually tested.

## Evidence in the pull request

Include a concise verification section in every PR:

1. The commands run and their results, tied to the submitted revision.
2. Regression coverage and any pre-fix reproduction.
3. Whether integration used synthetic fixtures or explicitly authorized live services.
4. Manual checks and tested platforms, including relevant UI evidence without private data.
5. Failures, skipped checks, and environment limitations, with the exact blocked stage and any baseline comparison.

Do not describe a blocked command as passing. Linux desktop checks require native GTK/WebKit dependencies; sandbox socket restrictions can block daemon tests. Report these precisely and use platform CI to establish missing coverage rather than suppressing assertions. The path-selected CI jobs and aggregate `Required checks` gate are described in [Development](development.md#pr-checks); the required gate must pass before merging.

Update this guide only when the verification procedure or current coverage changes. Do not append dated change logs, run counts, artifact inventories, or per-PR results.
