实现一个类似于codex  desktop 的agent 桌面应用，它能够连接远程服务器，打开远端的项目，然后开启远端的agent会话。
整体界面我希望能够和codex desktop几乎一样，但是简化很多功能。比如左边侧边栏，可能就分为聊天和项目两个分类，选择聊天后，可以看到历史聊天记录，选择以后继续聊，也可以选择新建对话。
而项目稍微复杂一点点，需要先创建一个项目（就是通过ssh连接上远程服务器，选择一个目录，或者选择一个本地目录）。然后在这个项目下可以创建“工作“，其实也就是开启agent会话。
聊天和工作的本质区别，我觉得就是，聊天的agent是没有机器目录访问权限的，它包含一些工具如webfetch等但不能对运行它的宿主环境产生影响；而工作则是我们更常见的vibe coding场景，需要选择目录，然后agent可以和环境交互去完成真正的任务，写代码等等等

有一个设置页面，用户可以在设置页面配置自己的供应商和模型，供应商按照协议分为3种（openai-completions/openai-responses/anthropic），每个供应商只支持一种协议，需要配baseurl，鉴权apikey，然后可以配置多个模型，每个模型还可以配置更细化的参数。

agent能力我希望基于copilot-sdk来建设，不自己实现agent。

技术栈要求：
1. 使用tauri v2做桌面端，目前只考虑支持mac和windows
2. 后端语言全使用rust(edition 2024 + 1.99.1)
3. 前端使用pnpm + React + Typescript(7.x) + vite(8.x) + 成熟的ui组件库

核心的agent部分我们使用copilot-sdk，基于它之上来开发能力，copilot-sdk版本先写死v1.0.23-preview.5
rust中的依赖，都用最新的版本，不要在cargo.toml里面固定小版本，方便随时升级修复依赖的bug

我有非常重的代码洁癖，我对架构设计的合理性优雅性有非常高的要求，对代码质量有非常高的要求，因此你至少需要:
1. 使用just命令把打包几个代码lint检查，每次按需执行这对于just命令，确保代码的质量红线
  1.1 小技巧，为了省token，可以rtk支持的命令前加上rtk，比如，原本一个命令是cargo build，可以改为rtk cargo build...
2. 质量红线包括：
  2.1 oxlint + oxfmt，因为从0开始，所有要尽可能提高要求增加合理的检查项
  2.2 cargo clippy, cargo +nightly fmt(imports_granularity = "Crate", group_imports = "StdExternalCrate")，clippy中尽量要求也要高一些，除了测试代码不要随意地allow某些规则
3. 单元测试尽量使用表格驱动的方式来写测试，且单测一定要有意义，不能为了写而写。要把测试代码当做需要维护的代码资产，如果价值不大还不如不写，一定要把测试的可读性放在第一位
4. 要按照agentsmd-creator skill所描述的方式来维护项目的文档，要把文档的重要性拔高。文档一定要简洁、主次分明、有逻辑条理，文档一定是给人读的，不追求多，细节越多反而越不好。要多参考how-to-write-tech-doc skill，同时要去ai味，参考de-ai-smell skill

其它一些细节，我列举一些：
1. 客户端和远端服务器通过ssh建立连接，建联之后会自动做一些环境初始化，比如安装我们的remote-server。像codex desktop，它就是ssh连上去之后，先启动服务器的codex app-server监听unix，然后启动codex app-server proxy，建立起桌面和unix socket之前的通信通道，从而实现桌面控制app-server。我们也可以借鉴。
2. copilot-sdk支持opentelemetry，应用需要支持让用户配置导出地址和导出协议等

你要以最大的努力来完成这个项目的开发，有问题自行探索或者参考行业其它类似应用的做法，如果有产品细节拿不准，着重参考codex desktop或者调研类似的应用，不要来问我，我需要的是结果

开发完了你需要自行验证，确保在当前机器可以配置provider和模型，然后可以进行聊天，也可以选择本地目录新建项目，然后开启工作来完成一些任务. 同时你要验证导出的opentelemetry数据是否正常
