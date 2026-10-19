# 开发与交付

先用 `just dev` 启动桌面。如果只改界面，在两个终端分别运行 `just server` 和 `just web`，浏览器打开 `http://127.0.0.1:1420`。两种运行方式共享服务契约；浏览器模式没有原生目录选择器时，使用内置目录浏览。

`just server` 启动的是 `relay-dev-server`，它通过 HTTP 调用与桌面相同的 `relay-core`。该命令会先构建 `relay-server`，以便在浏览器开发时也能测试 SSH 项目。`relay-server` 自身只提供远端 Agent 的 stdio 入口，不再承载开发 HTTP 服务。

workspace 的默认目标也是 `relay-dev-server`，因此 `cargo run` 仍启动本机 HTTP 主机；构建远端 Agent 时需显式选择 `-p relay-server`。

## 修改后运行什么

| 改动                     | 检查                                       |
| ------------------------ | ------------------------------------------ |
| React / TypeScript       | `just lint-web`、`pnpm test`、`pnpm build` |
| Rust 服务或 SDK 适配     | `just lint-rust`、`cargo test --workspace` |
| crate 依赖与服务入口     | `just check-workspace`                     |
| 验收脚本与事件等待       | `just test-python`                         |
| 格式                     | `just format`，再执行 `just check-format`  |
| 会话、工具、供应商或遥测 | `just integration`                         |
| SSH 传输、部署或远端协议 | `just integration-ssh`                     |
| 窗口、资源和原生功能     | `just desktop-build`，打开生成的应用验证   |

提交前运行 `just check`。oxlint 开启 React、TypeScript、可访问性与严格规则；TypeScript 使用 strict；Rust 运行 clippy all + pedantic，警告视为失败。格式使用 oxfmt 和 nightly rustfmt，后者按 crate 合并 import，并按标准库、外部库、当前 crate 分组。

单元测试针对状态合并、权限拒绝、配置边界、数据库关系、SSH 注入和通信失败。`just check` 也包含 Python 标准库测试，验证验收脚本自身的事件分流与进程清理，以及依赖边界检查器。集成测试使用真正的 SDK 与配套 CLI，只将外部推理服务与 OTLP 接收端替换为测试端点，因此不需要付费凭证。它不能证明某个真实供应商支持特定模型或参数。

## 真实供应商验收

`tests/live_provider_e2e.py` 用真实推理请求检查聊天、流式消息持久化，以及读取、创建、编辑文件和执行验证命令的工作流程。脚本固定连接 `https://camp-dev.woa.com/pingress`，需要能访问该网关并持有可用的供应商密钥。它会产生真实推理用量，不纳入日常质量门禁。

在仓库根目录运行：

```sh
# 先构建本机开发主机，脚本会自动启动并关闭它。
cargo build -p relay-dev-server

# 出现 Provider API key 提示后输入密钥，终端不会回显。
python3 tests/live_provider_e2e.py --output artifacts/live-provider-report.json
```

默认依次测试四个模型，每个模型分别执行聊天和工作用例：`claude-haiku-5.5`、`glm-5.3-flashx` 使用 Anthropic 协议；`gpt-6.1-sol`、`deepseek-v4.1-flash` 使用 OpenAI Responses 协议。本地观测代理将请求体原样转发到 HTTPS 网关，不修改模型或 token 参数。

密钥在交互终端中隐藏输入；非交互调用从标准输入读取一行，应由调用方通过受保护的输入通道提供，不要放进命令参数、环境变量或脚本文件。真实供应商验收显式使用临时数据目录和内存凭证库，不修改桌面应用的供应商配置，也不把真实密钥落盘；默认本地保存和重启恢复由使用测试密钥的 SDK 集成验证覆盖。工作审批仅允许临时项目内的文件访问、指定结果文件的写入和预先生成的验证命令。

重试单个或多个模型时，用 `--models` 列出模型 ID，并为本次运行保留独立报告：

```sh
python3 tests/live_provider_e2e.py --models deepseek-v4.1-flash --output artifacts/live-provider-deepseek-retry.json

# 只检查验收脚本自身的策略，不需要密钥，也不发送推理请求。
python3 tests/live_provider_e2e.py --self-test
```

每次重试都会创建新的隔离会话，并重跑所选模型的聊天和工作用例。`--output` 在每个用例结束后更新脱敏 JSON 报告；最终报告包含用例结果、请求数量、HTTP 状态、耗时、实际输出 token 参数，以及临时应用数据和日志的密钥扫描结果。请求观测不记录正文或鉴权头，错误文本中的密钥会被替换。进程退出码为 `0` 才表示本次所选用例及检查全部通过；单模型重试报告只代表该模型。

这里的 `contextWindow: 1000000` 表示输入和输出合计的总窗口。显式设置 `maxOutputTokens` 时，输入预算为总窗口减去该输出额度；例如两个 Anthropic 模型设置输出 `128000`，输入预算为 `872000`，并核对实际请求中的 `max_tokens`。OpenAI Responses 模型未显式设置输出额度，保留 SDK 默认值。短任务验收不证明接近 1M 窗口时的压缩行为；实际执行结果见[验收记录](verification.md)。

## 依赖和首次构建

`rust-toolchain.toml` 固定编译器版本；Cargo 依赖采用兼容主版本范围，0.x 库采用当前次版本。SDK 例外，使用要求的 Git tag。提交 `Cargo.lock` 与 `pnpm-lock.yaml`，升级后重新执行质量门禁。

SDK 首次编译需下载对应平台 CLI，断网环境应先准备构建缓存。若 Cargo 报镜像 TLS 或 DNS 错误，先检查本机 Cargo 源配置；不要为此放松证书校验。

服务入口的构建目标如下。调试本机应用时使用开发主机，准备远端分发时只构建远端 Agent。

| 命令                   | 构建目标与用途                                          |
| ---------------------- | ------------------------------------------------------- |
| `just build`           | 前端，以及 debug `relay-dev-server`、`relay-server`     |
| `just build-server`    | 两个 debug 服务二进制，供本机与 SSH 联调                |
| `just integration`     | 只构建 `relay-dev-server`，运行本地 SDK 验收            |
| `just integration-ssh` | 构建两个服务，启动本机开发主机并通过 SSH 部署远端 Agent |
| `just remote-build`    | 只构建 release `relay-server` 并复制到桌面资源目录      |

共享 RPC 定义放在 `relay-protocol`，避免远端为了读取请求而依赖整个 SSH 客户端或应用服务。修改依赖时同时检查[架构中的依赖边界](architecture.md#哪一层负责什么)，不要把开发 HTTP 主机、SQLite 或系统凭证库引入远端二进制。

`just check-workspace` 调用 `scripts/check_workspace.py`，读取 Cargo metadata 的普通依赖与构建依赖，验证七个 workspace crate 的单向关系，并检查 `relay-server` 的依赖闭包。它会拒绝本机应用服务、SSH 客户端、开发主机、Axum、SQLite、系统凭证库和 Tauri 进入远端 Agent。该检查纳入 `just check` 与 CI；测试专用依赖不作为远端分发依赖计算。

## 打包与远端产物

`just desktop-build` 先编译本机平台的远端 Agent `relay-server`，复制进 Tauri 资源目录，再打包桌面程序。`relay-dev-server` 不进入安装包。macOS 产物在 `target/release/bundle/`。默认使用 ad-hoc 本地签名；正式发布时覆盖 `bundle.macOS.signingIdentity` 并配置公证。发布签名和公证需要发布账户，仓库不包含凭证。

在锁屏或自动化环境中使用 `CI=true just desktop-build`，跳过需要 Finder 交互的 DMG 窗口装饰。

Linux 远端产物使用 `x86_64-unknown-linux-gnu` 或 `aarch64-unknown-linux-gnu`，macOS 使用对应 `apple-darwin` target。正式分发应包含所需远端平台的二进制，而非把桌面平台二进制发到不同系统。`scripts/prepare-remote.mjs <target>` 将显式 target 的 release 产物放到正确目录。

CI 的质量检查构建两个服务并运行本地 SDK 验收；Linux/macOS 远端任务只构建和上传 `relay-server`。桌面任务下载这些远端产物后打包，不把开发主机作为资源收集。

SSH 使用 BatchMode 和严格主机校验，适用于密钥与 ssh-agent。未知主机或需要交互登录时，先在终端完成登录。Windows 客户端需要系统 OpenSSH；Windows 作为远端执行主机暂不支持。

## 服务契约

桌面调用 `dispatch`，开发模式调用 `POST /api/dispatch`，请求为 `{ command, payload }`。类型定义集中在 `src/lib/types.ts` 和 Rust 的 `Request`；改动任一端都应更新另一端。流式事件通过 Tauri 的 `runtime-event` 或 `/api/events` 发送。

标题设置使用独立命令 `save_title_model {titleModel: {providerId, modelId} | null}`，`null` 表示跟随当前会话模型。`save_preferences` 保留该设置。`regenerate_title {sessionId}` 仅提交后台任务，完成通过 `title {title}` 事件通知，失败通过 `title_error {message}` 通知；二者都不改变会话运行状态。快照中的 `titleGenerations` 用于恢复生成进度。

供应商编辑页调用 `get_provider_api_key`（`id`）读取已保存密钥，返回字符串或 `null`；供应商不存在时返回错误。普通 `snapshot` 和 `save_provider` 响应仍只含 `hasApiKey`。保存请求省略 `apiKey` 保留原值，非空字符串替换，空字符串清除；密钥只写入本机 SQLite 的 `provider_secrets.secret`，不写入消息、日志或远端状态。开发主机 `--data-dir` 同时确定配置与密钥的存储目录；`--ephemeral-secrets` 可切换为进程内测试存储。

`send_message` 接受可选的 `messageId` UUID，保存后的用户消息沿用该 ID，便于界面在实时确认丢失后与历史准确合并。省略时由服务生成；相同 ID 的重复提交会被数据库拒绝，不会再次交给 Agent 执行。

工作会话的 `create_session` 可传 `permissionMode: "ask" | "allow-all"`，省略默认 `ask`。未运行的工作可通过 `set_permission_mode`（`sessionId`、`permissionMode`）切换，返回更新后的会话。聊天不能使用完全授权；运行中、初始化中和等待审批时拒绝切换。两套 SDK 集成验收都覆盖自动批准的真实文件和命令操作、切回询问后的拒绝，以及恢复会话后应用新模式；本地验收还覆盖应用服务重启后的模式持久化。

已有聊天或工作通过 `set_session_model {sessionId, providerId, modelId}` 切换模型，返回更新后的会话。允许选择任意已配置供应商中的模型；会话 ID、消息历史、标题、项目和工具权限保持不变。运行中、初始化中和等待审批时拒绝切换。选择与供应商外键同时持久化，旧 Agent 连接失效并断开；下一条消息恢复原 SDK 会话并应用新供应商、模型及其参数。切换请求即使被取消，也会完成旧连接清理；界面在结果未确认时阻止发送，并允许重新读取实际模型选择。

远端使用 `relay-protocol` 定义的逐行 JSON：请求 `{id,command,payload}`，响应 `{id,value}` 或 `{id,error:{code,message}}`，异步事件 `{event}`。这是 SSH 客户端与远端 Agent 的协议，与开发主机的 HTTP 命令入口分别维护。stdout 专用于协议，诊断输出写 stderr；请求内容和供应商凭证禁止写日志。
