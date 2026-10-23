set dotenv-load := false

python := if os() == "windows" { "python" } else { "python3" }

# List development commands and quality checks by default.
default:
    @just --list

# Install frontend and Python test dependencies; Rust uses the version in rust-toolchain.toml.
setup:
    {{python}} -m pip install -r tests/requirements.txt
    pnpm install --frozen-lockfile
    rustup toolchain install nightly --profile minimal --component rustfmt

# Start the complete desktop application.
dev:
    pnpm tauri dev

# For browser development, run just web in another terminal.
server:
    cargo build -p tandem-server
    cargo run -p tandem-dev-server

web:
    pnpm dev

format:
    pnpm format
    cargo +nightly fmt --all

check-format:
    pnpm format:check
    cargo +nightly fmt --all -- --check

lint-web:
    pnpm lint
    pnpm typecheck

lint-rust:
    cargo clippy --workspace --all-targets -- -D warnings

test: test-python
    pnpm test
    cargo test --workspace

test-python:
    {{python}} -m unittest discover -s tests -p 'test_*.py'

check-workspace:
    {{python}} scripts/check_workspace.py

check: check-format lint-web lint-rust test check-workspace

build:
    pnpm build
    cargo build -p tandem-dev-server -p tandem-server

# Build both the local development host and the SSH remote Agent.
build-server:
    cargo build -p tandem-dev-server -p tandem-server

# Use the real Copilot SDK with test inference and telemetry servers.
integration:
    cargo build -p tandem-dev-server
    {{python}} tests/runtime_e2e.py

# Verify AgentLens field mapping, content capture, credentials, retries, and remote export.
integration-agentlens: build-server
    {{python}} tests/agentlens_e2e.py

# Verify native trace and metric export over gRPC from local and remote sessions.
integration-grpc: build-server
    {{python}} tests/grpc_e2e.py

# Verify Git snapshots and review history through the real SDK and remote protocol.
integration-review: build-server
    {{python}} tests/review_acceptance.py

# Verify provider model discovery with local HTTP fixtures and synthetic credentials.
integration-models:
    cargo build -p tandem-dev-server
    {{python}} tests/provider_models_e2e.py

# Verify durable local statistics and remote usage events with synthetic inference.
integration-usage: build-server
    {{python}} tests/usage_dashboard_e2e.py

# Requires local ssh/sshd; all keys and host records stay in a temporary directory.
integration-ssh: build-server
    {{python}} tests/remote_e2e.py

# Prepare the Linux x64 remote Agent, using Docker or Podman on macOS and Windows.
remote-build:
    pnpm remote:build

# Verify prepared remote Agents without compiling them or invoking a container engine.
remote-check:
    pnpm remote:check

# Build the native desktop executable using the verified Linux remote Agent.
desktop-build:
    pnpm tauri build --no-bundle

# Prepare a changed or missing Linux remote Agent, then build the native desktop executable.
desktop-build-full: remote-build desktop-build

# Create native installers using the verified Linux remote Agent, as in release CI.
desktop-bundle:
    pnpm tauri build
