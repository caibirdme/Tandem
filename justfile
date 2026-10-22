set dotenv-load := false

python := if os() == "windows" { "python" } else { "python3" }

# List development commands and quality checks by default.
default:
    @just --list

# Install frontend dependencies; Rust uses the version in rust-toolchain.toml.
setup:
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

# Verify provider model discovery with local HTTP fixtures and synthetic credentials.
integration-models:
    cargo build -p tandem-dev-server
    {{python}} tests/provider_models_e2e.py

# Requires local ssh/sshd; all keys and host records stay in a temporary directory.
integration-ssh: build-server
    {{python}} tests/remote_e2e.py

# Bundle only the remote Agent; exclude the development HTTP host from resources.
remote-build:
    cargo build --release -p tandem-server
    node scripts/prepare-remote.mjs

desktop-build: remote-build
    pnpm tauri build
