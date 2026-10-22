# Tandem

## Project overview

Tandem is an agent desktop application for local directories and SSH projects. Chat exposes only web tools and automatically approves every tool permission request without prompting the user. Work runs in a selected directory with full access by default; the composer does not expose a permission selector. GitHub Copilot SDK provides the agent loop.

Stack: Tauri 2, Rust 1.99.1 / edition 2024, React, TypeScript 7, Vite 8, and pnpm. All backend business logic is written in Rust.

## Directory guide

| Path                        | Responsibility                                                                    |
| --------------------------- | --------------------------------------------------------------------------------- |
| `src/`                      | Desktop UI, interaction state, and typed transport                                |
| `src/i18n/`                 | Typed UI message catalogs and language selection                                  |
| `src-tauri/`                | Desktop windows, native directory selection, and command/event bridging           |
| `crates/tandem-core/`       | Project and session services, SQLite persistence, and local API key configuration |
| `crates/copilot-runtime/`   | Copilot SDK sessions, permission boundaries, and telemetry configuration          |
| `crates/tandem-remote/`     | OpenSSH connections, remote installation, and JSON RPC transport                  |
| `crates/tandem-protocol/`   | Shared SSH RPC frames, size limits, and errors                                    |
| `crates/tandem-server/`     | Remote shared daemon and SSH stdio proxy, distributed with desktop resources      |
| `crates/tandem-dev-server/` | Local HTTP development host that calls application services                       |
| `tests/`                    | Integration verification against real protocols and test services                 |
| `.github/workflows/`        | PR checks, desktop bundles, and tag releases                                      |
| `docs/`                     | Architecture, development, and verification guides                                |

## Documentation

- [README](README.md): installation, startup, and first use.
- [Architecture](docs/architecture.md): execution modes, permissions, and data flow.
- [Development](docs/development.md): quality gates, PR checks, and releases.
- [Verification](docs/verification.md): current checks, manual acceptance, and PR evidence requirements.
- [Original requirements](what-to-do.md): product scope and technical constraints.

## Development constraints

- Start every requested change in a separate Git worktree created from `main`, and develop and verify it there. Creating or switching branches in the current checkout alone is insufficient. Place worktrees under this repository's `.worktrees/` directory, for example `git worktree add -b codex/example .worktrees/example main`; `.gitignore` excludes this directory.
- Use English for all project documentation and project information, including code comments, commit message titles and bodies, PR titles and descriptions, issue text, release notes, logs, diagnostics, and default application messages. UI translations may use their target language inside locale catalogs. English is the default UI language; users can select another supported language in Settings. Keep UI text in typed catalogs so additional languages can be added consistently.
- Check official documentation before adding dependencies. Verify library and tool interfaces using the agreed Context7 `library` and `docs` steps.
- The project is at an early stage. Use current names, data schemas, and interfaces directly. Do not add backward-compatibility aliases, legacy data or configuration fallbacks, migration logic, or user upgrade instructions.
- Use `github-copilot-sdk = "1"` from crates.io. Use compatible major version ranges for other Rust dependencies; for 0.x dependencies, use the current compatible minor version without pinning the patch. Commit `Cargo.lock`.
- Core services must not depend on a window framework. Store API keys as requested in a separate configuration table in local SQLite, in plaintext. Read them on demand in the provider editor, hide them by default, and allow manual reveal. Ordinary snapshots must not contain keys. Never write keys to logs, browser storage, or command-line arguments. Do not use system credential stores.
- `tandem-protocol` contains only the shared protocol and must not depend on application services, SSH, or the SDK. The only permitted workspace dependencies of `tandem-server` are `copilot-runtime` and `tandem-protocol`; it must not include `tandem-core`, `tandem-remote`, HTTP services, SQLite, or system credential stores.
- `tandem-dev-server` is for local development and calls application services through `tandem-core`. Remote builds, SSH uploads, and desktop resources use only `tandem-server`; do not bundle the development host.
- Keep business rules in their owning modules. Transport layers only parse, call services, and forward events. Errors must provide actionable information.
- Define expected Rust failures as domain-specific enums derived with `thiserror`. Return typed `Result` values from services and match error variants for recovery and transport policy; never branch on error message strings. Enumerate distinct failure reasons instead of using string catch-all variants or `anyhow!`/`bail!` for business errors. Preserve underlying causes with `#[source]` or `#[from]`, attach structured context without secrets, and convert errors to display messages only at logging, UI, or serialization boundaries. Cross-process errors must retain stable, typed error codes. Reserve `anyhow` for test helpers when useful.
- Do not hide production code problems with lint suppressions. Enable TypeScript strict mode and run Rust Clippy with all + pedantic rules.
- Prioritize permissions, persistence, boundary inputs, and protocol failures in tests. Use table-driven cases for repeated structures; do not create tests merely to increase coverage numbers.
- Explain how users complete an action before describing the necessary design. Update relevant documentation whenever interfaces or installation procedures change.

## Delivery checks

- `just check`: formatting, static analysis, type checks, and tests all pass.
- `just check-workspace`: verify one-way workspace dependencies and ensure the remote binary excludes local services, databases, credential stores, and desktop frameworks.
- `just build`: compile the frontend, `tandem-dev-server`, and `tandem-server`.
- `just desktop-build`: successfully bundle the desktop application for the current platform.
- PRs must pass CI's `Required checks` before merging into `main`. When adding directories, build inputs, or check commands, update the path rules and steps in `.github/workflows/ci.yml`.
- Run the relevant integration verification for session, provider, or permission changes. Record commands, results, tested platforms, and limitations in the PR description, including whether real services or test services were used. Keep `docs/verification.md` as the current verification guide; do not append per-change logs or historical run results.
- Do not commit secrets, caches, build outputs, or local session data.
