# Tandem

一个用于聊天和项目工作的 Agent 桌面应用。选择自己的模型供应商，直接聊天；或者打开本地、SSH 目录，让 Agent 在项目中读文件、改代码、执行任务。

界面使用 React，桌面外壳使用 Tauri 2。Rust 服务通过固定版本的 GitHub Copilot SDK 驱动 Agent，无需 GitHub Copilot 订阅即可使用自己的 API Key。

## 启动

需要 Node.js 24、pnpm 12、Rust 1.99.1、just，以及当前系统的 [Tauri 开发依赖](https://v2.tauri.app/start/prerequisites/)。macOS 安装 Xcode Command Line Tools；Windows 安装 Visual Studio C++ Build Tools 和 WebView2。

```sh
just setup       # 安装前端依赖和 nightly rustfmt
just dev         # 打开桌面应用
```

第一次 Rust 编译会下载 SDK 配套的 Copilot CLI，并校验其校验和。之后运行时从应用内置资源提取，不要求用户另外安装 Copilot。SDK 固定为 `v1.0.23-preview.5`，对应 CLI `1.0.100-1`。

通用设置中的「Copilot CLI 路径」是调试用的可选覆盖项，日常使用保持为空。

## 从 Relay 升级

直接打开 Tandem 即可继续使用已有项目、供应商和会话。新安装使用 Tandem 的应用数据目录和 `tandem.sqlite3`；已有 Relay 数据时，应用会原地读取旧目录及数据库，包括 SDK 会话状态。若新旧目录都已有数据库，以 Tandem 目录为准，不自动合并数据。

Windows 不打安装包时，可运行 `pnpm tauri build --no-bundle`，再打开 `target/release/tandem-desktop.exe`。

## 第一次使用

1. 打开左下角「设置」，添加供应商。选择 OpenAI Completions、OpenAI Responses 或 Anthropic，填写服务地址和 API Key。
2. 添加供应商实际支持的模型 ID。显示名称可以自定；推理强度、上下文和输出限制按模型能力填写。
3. 新建聊天，选中模型并发送消息；首次发送会自动准备内置运行环境，无需先点击「准备运行环境」。聊天只提供网页读取工具，没有项目文件和命令权限。
4. 创建项目并选择一个本地目录，在项目下新建工作。在输入框旁选择权限模式：默认「逐次询问」，每次工具权限请求可允许或拒绝；「完全授权」会自动批准文件读写、命令执行等工具请求。

聊天或工作开始后，仍可点击输入框旁的模型名称，切换到任意已配置模型，也可更换供应商。选择保存后从下一条消息生效，保留同一段对话的历史、标题和工具权限。正在生成或等待工具审批时，先等待结束或点击停止，再切换模型；输入框中未发送的内容会保留。若连接中断导致选择未能确认，点击「重新确认模型」后再继续发送。

权限模式随工作会话保存，重新打开后继续使用。可在任务结束或失败后切换，正在执行或等待审批时不能更改；每次新建工作都从「逐次询问」开始。若首条消息发送前仍为逐次询问，会提示工具操作可能频繁打断，并提供「确认并继续」「切换到完全授权并继续」「取消」；取消或关闭弹窗会保留草稿。本地和 SSH 工作使用同样的规则。完全授权不是目录沙箱，命令仍可访问执行机器上当前用户有权访问的内容。

供应商地址要填写 API 基础地址，例如 `https://api.example.com/v1`，不要填写到具体的 `chat/completions` 或 `responses` 路径。API 密钥直接保存在本机配置中，重启后仍可使用。编辑供应商时，密钥默认隐藏；点击输入框旁的眼睛按钮可显示或再次隐藏，修改后点击保存即可。留空保留现有密钥，需要移除时使用「清除已保存的密钥」。

应用不使用系统钥匙串。从早期使用钥匙串的版本升级时，需要重新填写一次 API Key；本次名称变更会保留已保存在本机数据库中的密钥。

聊天和工作会在首轮回复完成后自动总结标题。在「设置 → 对话标题」中，可以从已配置的供应商模型中选择标题总结模型，也可跟随当前对话的模型。标题生成在后台进行，不阻塞任务；手动修改的标题会保留。已有会话可从选项菜单选择「重新生成标题」。

## SSH 项目

先在系统终端中完成一次 SSH 登录，确认主机指纹并配置密钥或 ssh-agent。Tandem 使用系统 OpenSSH 的配置和已信任主机记录，不保存登录密码。

在「新建项目」中选择 SSH，填写主机、端口、用户和可选私钥路径，再测试连接、选择远端目录。应用检测远端系统，将匹配的 `tandem-server` 上传到 `~/.local/share/tandem/bin/`，通过 SSH 的标准输入输出建立双向通道。远端不开放额外网络端口。

更名前的远端也会部署新的 `tandem-server`。SDK 状态默认放在 `~/.local/share/tandem/runtime`；该目录不存在而旧 `~/.local/share/relay/runtime` 已存在时，会原地使用旧状态以继续原会话。不会复用旧的 `relay-server` 程序。

桌面支持 macOS 和 Windows；远端支持 Linux 和 macOS。开发构建通过 `just remote-build` 准备本机平台的服务。连接不同平台时，将该平台服务放入 `src-tauri/remote/<Rust target>/tandem-server` 后打包，或用 `TANDEM_REMOTE_ARTIFACTS` 指定产物目录。CI 会分别构建 Linux/macOS 服务并放入桌面包。缺少匹配产物时，连接测试会明确报告目标平台。

## 遥测

设置中可启用 OpenTelemetry，填写 OTLP 接收地址，选择 `http/protobuf` 或 `http/json`。默认不采集提示词和回复正文；需要调试时再开启内容采集。设置更改会在当前对话停止后重启运行时，历史会话可以继续。

远端工作在远端导出遥测，因此接收地址必须从远端可达。`localhost` 指的是执行 Agent 的机器。

## 开发和验收

测试还需要 Python 3.9 或更新版本，只使用标准库。

只改界面时，在两个终端分别运行 `just server` 和 `just web`。前者启动本机 HTTP 开发主机 `tandem-dev-server`，并先构建 SSH 所需的远端 Agent。上传到远端和放进桌面资源的始终是 `tandem-server`；开发主机不随应用分发。

```sh
just check          # 格式、类型、lint 和单元测试
just integration    # 真实 SDK/CLI 与可控供应商的端到端验证
just desktop-build  # 打包当前平台桌面应用及本机远端服务
```

[架构](docs/architecture.md)介绍模块边界与权限；[开发说明](docs/development.md)介绍质量门禁和打包；[验收记录](docs/verification.md)区分已验证行为和需要真实服务的检查。
