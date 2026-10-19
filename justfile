set dotenv-load := false

python := if os() == "windows" { "python" } else { "python3" }

# 默认展示开发入口和质量门禁。
default:
    @just --list

# 安装前端依赖；Rust 使用 rust-toolchain.toml 中的版本。
setup:
    pnpm install --frozen-lockfile
    rustup toolchain install nightly --profile minimal --component rustfmt

# 启动完整桌面应用。
dev:
    pnpm tauri dev

# 浏览器开发：另一个终端执行 just web。
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

# 同时构建本机开发主机与 SSH 远端 Agent。
build-server:
    cargo build -p tandem-dev-server -p tandem-server

# 使用真实 Copilot SDK；推理服务和遥测接收端由测试提供。
integration:
    cargo build -p tandem-dev-server
    {{python}} tests/runtime_e2e.py

# 需要本机 ssh/sshd；所有密钥和主机记录均隔离在临时目录。
integration-ssh: build-server
    {{python}} tests/remote_e2e.py

# 只打包远端 Agent，开发 HTTP 主机不进入分发资源。
remote-build:
    cargo build --release -p tandem-server
    node scripts/prepare-remote.mjs

desktop-build: remote-build
    pnpm tauri build
