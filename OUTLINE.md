# Joker 项目总纲

## 1. 项目定位

Joker 是一个内核精简、API 封装优先、强自定义化的 Rust 编码代理框架。它的核心目标不是先做一个功能堆满的终端产品，而是提供一套足够小、足够清晰、可组合的 agent kernel，让开发者可以通过 Rust API 或轻量配置自由组合：

- 模型 Provider：Scripted、本地 OpenAI-compatible、DeepSeek、Anthropic、Google 等。
- 工具集合：`read`、`write`、`web_search`、`shell`、`grep`、`apply_patch`、MCP 工具等。
- 工具权限：自动批准、调用时请求批准、拒绝、只读默认信任、会话级批准、项目级持久批准。
- 上下文构建：原始对话、固定窗口、摘要压缩、项目上下文、规则/记忆注入。
- 运行宿主：库 API、TUI、CLI one-shot、后续可扩展到 HTTP/ACP/MCP server。

一句话目标：**Joker 应该像一个“可编程 agent 运行内核”，而不是只能按固定产品形态运行的 CLI。**

## 2. 调研结论

对 `../agents/analysis` 中 Codex、Claude Code、OpenCode、Gemini CLI、pi、CodeWhale、DeepSeek-Reasonix、MiMo-Code、oh-my-openagent 等项目的分析，可以抽取出几个对 Joker 最有价值的共性：

1. **核心 loop 必须小而稳定**：成熟项目都把模型流、工具调用、权限拦截、事件输出作为稳定主轴，复杂功能尽量挂在外围。
2. **工具和权限必须分离**：工具只声明能力与风险，策略层决定是否允许执行。Codex、Gemini CLI、CodeWhale 都有多层权限/沙箱/审批模型。
3. **Provider 适配层不能污染内核**：OpenCode、pi、Claude Code 都通过统一模型事件格式屏蔽不同厂商差异。
4. **会话和事件是后续扩展基础**：OpenCode/Codex/pi 都把 session、thread store、事件流作为恢复、压缩、UI、协议集成的基础。
5. **扩展点比内置功能更重要**：MCP、插件、skills、hooks 的价值在于让用户自定义 agent，而不是把所有能力写死在主程序里。

Joker 的取舍：不直接复制大型项目的重架构，而是保留这些项目验证过的核心边界，用小 crate 和 trait API 逐步长出能力。

## 3. 当前实现状态

当前仓库已经具备一个可运行的最小 agent kernel 和 TUI demo。`cargo test` 已通过，覆盖 agent loop、工具调用、Provider 转换、配置、TUI 状态与渲染。

| 板块 | 状态 | 当前实现 |
|---|---:|---|
| Agent 主循环 | 已实现 | `Agent::run` 支持多步模型调用、工具调用回填、取消、步数/工具数限制 |
| Model trait | 已实现 | `Model::stream` 抽象统一流式模型事件 |
| Provider 适配 | 部分实现 | Scripted、OpenAI-compatible、DeepSeek/Alibaba/Zhipu/Moonshot/Baidu preset、Anthropic、Google |
| Tool trait/registry | 已实现 | `Tool`、`ToolRegistry`、JSON schema、timeout、并行安全标注 |
| 只读工具 | 已实现 | `list_files`、`read_file`、`grep`，带 workspace path 防逃逸 |
| 权限策略 | 雏形 | `AllowAllPolicy`、`DenyAllMutatingPolicy`；尚无交互式审批和持久规则 |
| 上下文构建 | 雏形 | passthrough、fixed window、基础 byte/message limit |
| 事件观察 | 已实现 | run/model/tool/limit 事件，TUI 通过 channel 接收 |
| TUI | Demo 可用 | transcript、输入框、流式输出、工具状态、provider/model 选择弹窗 |
| Slash command | 部分实现 | `/help`、`/clear`、`/quit`、`/cancel`、`/status`、`/provider`、`/model`、`/models`、`/config`、`/tools` |
| 配置系统 | 部分实现 | `joker.toml`、CLI override、运行时切换 provider/model、保存配置 |
| 会话持久化 | 未实现 | 当前 conversation 只存在单次 run/TUI 内存状态 |
| 写文件/补丁工具 | 未实现 | 尚无 `write_file`、`edit_file`、`apply_patch` |
| Shell 工具 | 未实现 | 尚无命令解析、sandbox、approval |
| Web search 工具 | 未实现 | 尚无 web provider trait 和工具封装 |
| MCP/插件/skills/hooks | 未实现 | 仅有内核扩展点雏形，无动态发现/加载 |

## 4. 核心 API 设计方向

Joker 的首要产品形态应该是库 API。TUI 只是一个宿主示例。

目标 API 形态：

```rust
let agent = AgentBuilder::new(model)
    .system_prompt("You are a repository maintenance agent.")
    .tools(
        ToolSet::new()
            .read()
            .grep()
            .write()
            .shell()
            .web_search(),
    )
    .permissions(
        PermissionPolicy::new()
            .auto_approve("read_file")
            .auto_approve("grep")
            .ask("write_file")
            .ask("shell")
            .deny("shell", CommandPattern::dangerous()),
    )
    .context(FixedWindowContextBuilder::new(64))
    .observer(observer)
    .build();
```

配置式目标：

```toml
[agent.default]
model = "deepseek/deepseek-v4-flash"
system = "You are a coding agent."

[agent.default.tools]
read_file = { enabled = true, permission = "auto" }
grep = { enabled = true, permission = "auto" }
write_file = { enabled = true, permission = "ask" }
shell = { enabled = true, permission = "ask" }
web_search = { enabled = true, permission = "auto" }

[agent.default.permissions]
remember_session_approvals = true
deny = ["shell:rm -rf *", "shell:sudo *"]
```

## 5. 工具系统规划

工具的职责是声明能力、输入 schema、输出格式和风险元数据，不直接处理用户审批。

| 工具 | 目标能力 | 权限默认值 | 状态 |
|---|---|---|---:|
| `list_files` | 列目录 | auto | 已实现 |
| `read_file` | 读取 UTF-8 文件，支持 max bytes | auto | 已实现 |
| `grep` | 工作区文本搜索 | auto | 已实现 |
| `write_file` | 创建/覆盖文件 | ask | 未实现 |
| `edit_file` | 局部替换、结构化编辑 | ask | 未实现 |
| `apply_patch` | 应用 unified patch | ask | 未实现 |
| `shell` | 执行命令并返回 stdout/stderr/exit code | ask | 未实现 |
| `web_search` | 搜索网页，返回摘要和链接 | auto 或 ask | 未实现 |
| `fetch_url` | 拉取网页/文档内容 | auto 或 ask | 未实现 |
| `todo_write` | 写入 agent 内部任务列表 | auto | 未实现 |
| `memory_read` | 读取项目/用户记忆 | auto | 未实现 |
| `memory_write` | 写入记忆 | ask | 未实现 |
| `mcp_*` | 外部 MCP server 工具代理 | 按 server/tool 配置 | 未实现 |

工具元数据需要补齐：

- `mutating`: 是否修改外部状态。
- `network`: 是否访问网络。
- `workspace_scoped`: 是否限制在工作区内。
- `command_like`: 是否执行 shell/子进程。
- `permission_key`: 用于策略匹配，例如 `shell:git status`、`mcp_github_create_issue`。
- `display`: 给 TUI/审批弹窗展示的摘要。

## 6. 权限系统规划

权限系统是 Joker 的核心差异化能力。目标是支持“每个工具如何批准”可以被组合配置，而不是写死在工具里。

权限决策类型：

| 决策 | 含义 | 状态 |
|---|---|---:|
| `allow` | 直接执行 | 已有雏形 |
| `deny` | 拒绝执行并把错误结果回填给模型 | 已有雏形 |
| `ask_once` | 本次调用请求用户批准 | 未实现 |
| `allow_for_session` | 本会话同类调用自动批准 | 未实现 |
| `allow_persisted` | 写入项目/用户权限配置，后续自动批准 | 未实现 |
| `rewrite` | 策略层改写参数，例如追加 sandbox/env | 未实现 |

策略匹配维度：

- 按工具名：`read_file`、`write_file`、`shell`。
- 按工具分类：只读、写入、网络、命令执行。
- 按路径：允许 `docs/**` 写入，禁止 `.git/**`、`target/**`。
- 按命令前缀：允许 `cargo test`、`git status`，询问 `git commit`，拒绝 `rm -rf`。
- 按 MCP 命名空间：`mcp_github_*`、`mcp_figma_get_file`。
- 按 agent profile：不同自定义 agent 拥有不同工具和权限。

优先级建议：

```text
hard deny
> persisted allow/deny
> session allow/deny
> agent profile rule
> tool annotation default
> global default
```

## 7. Slash Command 规划

Slash command 是 TUI/CLI 宿主层能力，不应绑死核心内核。但它需要能驱动自定义 agent 的配置和审批。

| 命令 | 目标 | 状态 |
|---|---|---:|
| `/help` | 查看命令 | 已实现 |
| `/clear` | 清空 transcript | 已实现 |
| `/quit` | 退出 | 已实现 |
| `/cancel` | 取消当前 run | 已实现 |
| `/status` | 查看 provider/model/running/tools/config | 已实现 |
| `/provider` | 切换 provider | 已实现 |
| `/model` | 切换 model | 已实现 |
| `/models` | 查看当前 provider 模型 | 已实现 |
| `/config show/set/save` | 查看、临时修改、保存配置 | 部分实现 |
| `/tools` | 查看启用工具 | 已实现 |
| `/agent` | 切换/查看 agent profile | 未实现 |
| `/permissions` | 查看/编辑工具权限规则 | 未实现 |
| `/approve` / `/deny` | 响应挂起的工具审批 | 未实现 |
| `/sessions` | 列出/恢复会话 | 未实现 |
| `/compact` | 压缩上下文 | 未实现 |
| `/memory` | 查看/写入记忆 | 未实现 |
| `/mcp` | 管理 MCP server | 未实现 |
| `/web` | 管理 web search provider | 未实现 |

## 8. 自定义 Agent Profile

Joker 应该支持用同一内核装配多个自定义 agent。每个 profile 至少包含：

- `name` / `description`
- `system_prompt`
- `model` / `provider`
- `tools`
- `permissions`
- `context_policy`
- `working_directory`
- `environment`
- `hooks`

示例 profiles：

| Profile | 工具组合 | 权限策略 |
|---|---|---|
| `reader` | `list_files`、`read_file`、`grep`、`web_search` | 全部 auto，只读 |
| `coder` | `read_file`、`grep`、`write_file`、`apply_patch`、`shell` | 读 auto，写/shell ask |
| `docs` | `read_file`、`grep`、`write_file`、`web_search` | docs 路径写入 ask，网络 auto |
| `ci-fixer` | `read_file`、`grep`、`apply_patch`、`shell:cargo test` | `cargo test` auto，写 ask |
| `researcher` | `web_search`、`fetch_url`、`read_file`、`memory_write` | search auto，memory write ask |

## 9. 阶段路线图

### P0：保住精简内核，完成可配置工具/权限骨架

状态：进行中。

- 已完成：`Model`、`Tool`、`ToolRegistry`、`ToolPolicy`、`ContextBuilder`、`Observer` 等核心 trait。
- 已完成：只读 workspace 工具。
- 待实现：`AgentBuilder` / `ToolSet` / `PermissionPolicy` 的友好 API。
- 待实现：工具权限配置模型与 TOML 解析。
- 待实现：TUI 中工具审批的 pending request 状态机。

验收标准：

- 用户可以通过 Rust API 创建一个只读 agent、一个可写 agent。
- 同一个工具在不同 agent profile 下可以有不同审批策略。
- 未批准的 mutating 工具不会执行，模型会收到结构化拒绝结果。

### P1：补齐编码 agent 必需工具

状态：未开始。

- `write_file`
- `edit_file`
- `apply_patch`
- `shell`
- shell 命令安全分析：命令链、路径逃逸、危险前缀、环境变量控制。
- workspace-scoped 写入保护。

验收标准：

- 默认配置下读工具自动执行，写入/shell 必须请求批准。
- TUI 能展示工具参数摘要，并允许 approve/deny。
- `cargo test`、`git status` 等低风险命令可通过规则自动批准。

### P2：Web/Search 和网络工具

状态：未开始。

- `WebSearch` trait，避免绑定单一搜索服务。
- `web_search` tool。
- `fetch_url` tool。
- 网络权限分类：auto/ask/deny。
- 搜索结果 citation/schema 标准化。

验收标准：

- agent profile 可以选择启用或禁用网络工具。
- 搜索工具输出稳定结构，便于模型引用。

### P3：会话、上下文压缩、记忆

状态：未开始。

- `SessionStore` trait：memory/file/jsonl 后端。
- transcript 持久化和恢复。
- `/sessions`、`/compact`。
- summary-based context builder。
- `memory_read` / `memory_write`。

验收标准：

- TUI 退出后可以恢复上一会话。
- 长上下文触发压缩后仍保留关键任务状态。

### P4：扩展生态

状态：未开始。

- MCP client：发现外部工具并映射到 Joker `Tool`。
- MCP server：让 Joker 暴露自身工具。
- Hook 系统：run/tool/model/session 生命周期事件。
- Skills/插件目录：加载额外 prompt、工具、权限规则。
- HTTP/ACP 宿主：把同一 agent kernel 暴露给其他前端。

验收标准：

- 外部 MCP 工具可按命名空间配置权限。
- 插件/skill 可以注册工具和默认规则，但不能绕过用户权限策略。

## 10. 架构边界

建议继续保持 crate 边界清晰：

| Crate | 职责 | 调整方向 |
|---|---|---|
| `joker` | 纯 agent kernel：trait、协议、loop、策略接口 | 增加 builder/profile/permission request 抽象 |
| `joker-tools` | 内置工具实现 | 拆分 readonly、mutating、network、shell 工具模块 |
| `joker-provider` | Provider 适配 | 继续只输出统一 `ModelResponseEvent` |
| `joker-config` | 文件配置到 runtime/profile 的解析 | 增加 agent profile、tools、permissions 配置 |
| `joker-tui` | 一个宿主实现 | 增加审批 UI、profile 切换、session 管理 |

原则：

- 核心 crate 不依赖 TUI、HTTP、具体 Provider、具体搜索服务。
- 工具实现不直接询问用户，询问必须通过 policy/host 完成。
- 权限策略不绑定 TUI，CLI/HTTP 也能复用。
- 配置是 API 的投影，不能替代 Rust API 的可组合性。

## 11. 当前最高优先级

下一步最应该做的是：

1. 在 `joker` 中扩展 `ToolPolicy`，让它能返回 `Ask`/`PendingApproval`，而不只是 `Allow/Deny`。
2. 定义 `PermissionPolicy` 和 `ToolPermissionRule`，支持按工具名、工具分类、路径、命令前缀匹配。
3. 在 `joker-config` 中加入 agent profile、tools、permissions 配置结构。
4. 在 `joker-tui` 中实现工具审批 pending 状态和 approve/deny 交互。
5. 再实现 `write_file` / `apply_patch` / `shell`，确保 mutating 工具从第一天就走审批流程。

这条路线能保证 Joker 的 demo 不是“只有 TUI 聊天”，而是真正展示：**同一个精简内核如何用工具和权限组合出不同的自定义编码代理。**
