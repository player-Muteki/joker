# Joker 深度分析报告

## 项目画像

| 维度 | 描述 |
|------|------|
| **项目名称** | Joker |
| **本质** | 小型的可定制 Rust TUI Agent 内核 |
| **技术栈** | Rust 2024, tokio, ratatui, crossterm, reqwest |
| **代码规模** | ~4,500 行 Rust, 5 个 workspace crate |
| **架构风格** | Trait 驱动的插件式架构 |
| **构建工具** | Cargo workspace |
| **关键协议** | SSE (Server-Sent Events), JSON-RPC over HTTP |
| **界面** | TUI (ratatui) + CLI (clap) |
| **运行时** | tokio 异步运行时 |

## 分析索引

| # | 文件名 | 覆盖主题 |
|---|--------|---------|
| 1 | [01-overview.md](01-overview.md) | 项目概览、能力边界、设计哲学 |
| 2 | [02-architecture.md](02-architecture.md) | 架构总览、Crate 依赖关系、模块划分 |
| 3 | [03-entrypoint.md](03-entrypoint.md) | 入口与启动流程 (main → cli → tui) |
| 4 | [04-agent-loop.md](04-agent-loop.md) | Agent 主循环 (Agent::run 分析) |
| 5 | [05-protocol-types.md](05-protocol-types.md) | 核心协议类型: Conversation, Message, Content, Role, ToolCall |
| 6 | [06-model-trait.md](06-model-trait.md) | Model trait + 三 Provider 实现 (OpenAI/Anthropic/Google) |
| 7 | [07-tools-system.md](07-tools-system.md) | Tool trait + 内置工具 + 安全机制 |
| 8 | [08-context-policy.md](08-context-policy.md) | ContextBuilder + ToolPolicy 系统 |
| 9 | [09-event-observer.md](09-event-observer.md) | 事件系统 (Event enum, Observer trait, ChannelObserver) |
| 10 | [10-tui-architecture.md](10-tui-architecture.md) | TUI 架构 (App 状态机, 事件循环, Widgets, 命令系统) |
| 11 | [11-config-system.md](11-config-system.md) | 配置系统 (joker.toml, RuntimeConfig, ConfigOverrides, CLI) |
| 12 | [12-file-index.md](12-file-index.md) | 核心文件索引 |
| 13 | [13-design-principles.md](13-design-principles.md) | 设计原则提炼 |
| 14 | [14-key-classes.md](14-key-classes.md) | 核心类型深度分析 |
