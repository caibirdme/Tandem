# Tandem

## 项目全貌

Tandem 是一个连接本地目录和 SSH 项目的 Agent 桌面应用。聊天只提供网页工具，所有工具调用自动授权，不询问用户；工作在选定目录中执行，默认逐次询问权限，也可由用户为该会话选择完全授权。Agent 循环由固定版本的 GitHub Copilot SDK 提供。

技术栈：Tauri 2、Rust 1.99.1 / edition 2024、React、TypeScript 7、Vite 8、pnpm。后端业务逻辑全部使用 Rust。

## 目录

| 路径                        | 职责                                        |
| --------------------------- | ------------------------------------------- |
| `src/`                      | 桌面界面、交互状态和类型化传输              |
| `src-tauri/`                | 桌面窗口、系统目录选择、命令和事件桥接      |
| `crates/tandem-core/`       | 项目与会话服务、SQLite 持久化、本地密钥配置 |
| `crates/copilot-runtime/`   | Copilot SDK 会话、权限边界和遥测配置        |
| `crates/tandem-remote/`     | OpenSSH 建联、远端安装和 JSON RPC 通道      |
| `crates/tandem-protocol/`   | SSH 两端共享的 JSON RPC 帧、大小限制与错误  |
| `crates/tandem-server/`     | 远端 Agent 的 stdio 入口，随桌面资源分发    |
| `crates/tandem-dev-server/` | 本机 HTTP 开发主机，调用应用服务            |
| `tests/`                    | 真实协议集成验证及测试服务                  |
| `docs/`                     | 架构、开发和验收记录                        |

## 文档入口

- [README](README.md)：安装、启动和首次使用。
- [架构](docs/architecture.md)：运行方式、权限和数据流。
- [开发](docs/development.md)：质量门禁与打包。
- [验收](docs/verification.md)：已执行验证及其边界。
- [原始需求](what-to-do.md)：产品范围与技术约束。

## 开发约束

- 所有需求改动必须从 `main` 创建独立 Git worktree，在该 worktree 中开发和验证，不得仅在当前工作区直接新建或切换分支。worktree 统一放在本仓库根目录的 `.worktrees/` 下，例如 `git worktree add -b fix/example .worktrees/example main`；该目录由 `.gitignore` 忽略。
- 新依赖先查官方文档；库和工具接口按用户约定使用 Context7 的 `library`、`docs` 两步查证。
- Copilot SDK 固定为 `v1.0.23-preview.5`。其他 Rust 依赖使用主版本兼容范围；0.x 依赖使用当前兼容次版本，不锁补丁号。`Cargo.lock` 必须提交。
- 核心服务不依赖窗口框架。API key 按用户要求明文保存在本机 SQLite 的独立配置表；编辑页按需读取，默认隐藏并允许手动显示。普通快照不含密钥，禁止写入日志、浏览器存储和命令行参数。不使用系统凭证库，不实现旧版密钥迁移。
- `tandem-protocol` 只负责共享协议，不依赖应用服务、SSH 或 SDK。`tandem-server` 的 workspace 依赖只允许 `copilot-runtime` 与 `tandem-protocol`，不能引入 `tandem-core`、`tandem-remote`、HTTP 服务、SQLite 或系统凭证库。
- `tandem-dev-server` 只用于本机开发，通过 `tandem-core` 调用应用服务。远端构建、SSH 上传与桌面资源只使用 `tandem-server`，不打包开发主机。
- 业务规则集中在所属模块；传输层只做解析、调用和事件转发。错误必须带可操作的信息。
- 禁止用 lint 抑制掩盖生产代码问题。TypeScript 开启 strict，Rust 运行 clippy all + pedantic。
- 测试优先覆盖权限、持久化、边界输入和协议失败；多组相同结构用表格驱动，不以覆盖率数字制造测试。
- 文档先讲用户如何完成一个动作，再解释必要设计。接口或安装方式变化时同时更新相关文档。

## 交付检查

- `just check`：格式、静态检查、类型和测试全部通过。
- `just check-workspace`：验证 workspace 单向依赖，以及远端二进制不携带本机服务、数据库、凭证库或桌面框架。
- `just build`：前端、`tandem-dev-server` 和 `tandem-server` 编译通过。
- `just desktop-build`：当前平台桌面打包通过。
- 会话、供应商或权限变更，执行对应集成验证；在验收记录中写清使用真实服务还是测试服务。
- 不提交密钥、缓存、构建产物和本地会话数据。
