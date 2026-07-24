# Joker 项目总纲

## 1. 项目定位

Joker 是一个内核精简、API 封装优先、强自定义化的 Rust 编码代理框架。它的核心目标不是先做一个功能堆满的终端产品，而是提供一套足够小、足够清晰、可组合的 agent kernel，让开发者可以通过 Rust API 或轻量配置自由组合：

- 模型 Provider：OpenAI-compatible、DeepSeek、Anthropic、Google 等。
- 工具集合：`read`、`write`、`web_search`、`shell`、`grep`、`apply_patch`、MCP 工具等。
- 工具权限：自动批准、调用时请求批准、拒绝、只读默认信任、会话级批准、项目级持久批准。
- 上下文构建：原始对话、固定窗口、摘要压缩、项目上下文、规则/记忆注入。
- 运行宿主：库 API、TUI、CLI one-shot、后续可扩展到 HTTP/ACP/MCP server。

一句话目标：**Joker 应该像一个"可编程 agent 运行内核"，而不是只能按固定产品形态运行的 CLI。**

## 2. 调研结论

对 `../agents/analysis` 中 Codex、Claude Code、OpenCode、Gemini CLI、pi、CodeWhale、DeepSeek-Reasonix、MiMo-Code、oh-my-openagent 等项目的分析，可以抽取出几个对 Joker 最有价值的共性：

1. **核心 loop 必须小而稳定**：成熟项目都把模型流、工具调用、权限拦截、事件输出作为稳定主轴，复杂功能尽量挂在外围。
2. **工具和权限必须分离**：工具只声明能力与风险，策略层决定是否允许执行。Codex、Gemini CLI、CodeWhale 都有多层权限/沙箱/审批模型。
3. **Provider 适配层不能污染内核**：OpenCode、pi、Claude Code 都通过统一模型事件格式屏蔽不同厂商差异。
4. **会话和事件是后续扩展基础**：OpenCode/Codex/pi 都把 session、thread store、事件流作为恢复、压缩、UI、协议集成的基础。
5. **扩展点比内置功能更重要**：MCP、插件、skills、hooks 的价值在于让用户自定义 agent，而不是把所有能力写死在主程序里。

Joker 的取舍：不直接复制大型项目的重架构，而是保留这些项目验证过的核心边界，用小 crate 和 trait API 逐步长出能力。

## 3. 当前实现状态

当前仓库已经具备一个可运行的最小 agent kernel、TUI demo、AgentBuilder/ToolSet/PermissionPolicy API、写入/Shell/Web 工具、会话持久化与记忆工具。cargo test 已通过（62 tests），覆盖 agent loop、工具调用、Provider 转换、配置、TUI 状态、session 持久化、写入工具和网络工具。

| 板块 | 状态 | 当前实现 |
|---|---:|---|
| Agent 主循环 | 已实现 | Agent::run 多步模型调用、工具调用回填、取消、步数/工具数限制 |
| AgentBuilder | 已实现 | Fluent API：model、tools、permissions、approval_channel、system_prompt、build |
| ToolSet | 已实现 | read、grep、write、shell、web_search 五类选择 + has_* 访问器 |
| Model trait | 已实现 | Model::stream 统一流式模型事件，ModelFuture 异步生命周期 |
| Provider 适配 | 部分实现 | Scripted、OpenAI-compatible、DeepSeek、Alibaba、ZhipuAI、Moonshot、Baidu、Anthropic、Google |
| Tool trait/registry | 已实现 | Tool、ToolRegistry、JSON schema、timeout、并行安全标注 |
| 只读工具 | 已实现 | list_files、read_file、grep，resolve_read workspace 防逃逸 |
| 写入/补丁工具 | 已实现 | write_file（创建/覆盖）、edit_file（首次匹配替换）、apply_patch（unified diff 解析） |
| Shell 工具 | 已实现 | sh -c 执行 + analyse_command_safety（命令链、路径逃逸、LD_PRELOAD、trusted prefix） |
| Web search 工具 | 已实现 | WebSearch trait + DuckDuckGoSearch 后端（免 API key）+ WebSearchTool 封装 |
| Fetch URL 工具 | 已实现 | fetch_url，私有 IP 阻断 + HTML 纯文本提取 |
| 记忆工具 | 已实现 | memory_read、memory_write，.joker-memory/ 文件存储 |
| 权限策略 | 已实现 | AllowAllPolicy、DenyAllMutatingPolicy、PermissionPolicy（RulePattern + ToolCategory + Ask） |
| 审批通道 | 已实现 | SharedApprovalChannel（submit/respond/take_response/pending_request）+ ToolDecision::Ask |
| 上下文构建 | 部分实现 | passthrough、fixed window、SummaryContextBuilder（摘要压缩）、基础 byte/message limit |
| 事件观察 | 已实现 | run/model/tool/limit delta/finished 事件，TUI 通过 channel 接收 |
| TUI | Demo 可用 | transcript、输入框、流式输出、工具状态、provider/model 弹窗、审批 approve/deny |
| Slash command | 部分实现 | /help、/clear、/quit、/cancel、/status、/provider、/model、/models、/config、/tools、/sessions、/compact |
| 配置系统 | 已实现 | joker.toml + CLI override + 运行时切换 + AgentProfileConfig/PermissionRuleConfig/ToolPermissionConfig |
| 会话持久化 | 已实现 | SessionStore trait + JsonlSessionStore（JSONL 文件后端，save/load/list/delete） |
| MCP/插件/skills/hooks | 未实现 | 仅有内核扩展点雏形，无动态发现/加载 |

## 4. 核心 API 设计方向

Joker 的首要产品形态是库 API。TUI 只是一个宿主示例。

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
| list_files | 列目录 | auto | 已实现 |
| read_file | 读取 UTF-8 文件，支持 max_bytes | auto | 已实现 |
| grep | 工作区文本搜索 | auto | 已实现 |
| write_file | 创建/覆盖文件 | ask | 已实现 |
| edit_file | 局部替换、结构化编辑 | ask | 已实现 |
| apply_patch | 应用 unified patch | ask | 已实现 |
| shell | 执行命令并返回 stdout/stderr/exit code | ask | 已实现 |
| web_search | 搜索网页，返回摘要和链接 | auto 或 ask | 已实现（DuckDuckGo） |
| fetch_url | 拉取网页/文档内容 | auto 或 ask | 已实现 |
| memory_read | 读取项目/用户记忆 | auto | 已实现 |
| memory_write | 写入记忆 | ask | 已实现 |
| todo_write | 写入 agent 内部任务列表 | auto | 未实现 |
| mcp_* | 外部 MCP server 工具代理 | 按 server/tool 配置 | 未实现 |

工具元数据：

- mutating：是否修改外部状态。
- network：是否访问网络。
- workspace_scoped：是否限制在工作区内。
- command_like：是否执行 shell/子进程。
- permission_key：用于策略匹配，例如 shell:git status、mcp_github_create_issue。
- display：给 TUI/审批弹窗展示的摘要。

## 6. 权限系统规划

权限系统是 Joker 的核心差异化能力。目标是支持"每个工具如何批准"可以被组合配置，而不是写死在工具里。

权限决策类型：

| 决策 | 含义 | 状态 |
|---|---|---:|
| allow | 直接执行 | 已实现 |
| deny | 拒绝执行并把错误结果回填给模型 | 已实现 |
| ask_once | 本次调用请求用户批准 | 已实现 |
| allow_for_session | 本会话同类调用自动批准 | 已实现 |
| allow_persisted | 写入项目/用户权限配置，后续自动批准 | 部分实现（数据结构已定义） |
| rewrite | 策略层改写参数，例如追加 sandbox/env | 未实现 |

策略匹配维度：

- 工具名精确匹配。
- ToolCategory 分类匹配（Read / Write / Shell / Network）。
- 路径前缀匹配（docs/、.git/ 等）。
- 命令前缀匹配（cargo test 等）。
- MCP 命名空间匹配。
- Agent profile 级别匹配。

优先级：

```
hard deny
> persisted allow/deny
> session allow/deny
> agent profile rule
> tool annotation default
> global default
```

## 7. Slash Command 规划

Slash command 是 TUI/CLI 宿主层能力，不应绑死核心内核。

| 命令 | 目标 | 状态 |
|---|---|---:|
| /help | 查看命令 | 已实现 |
| /clear | 清空 transcript | 已实现 |
| /quit | 退出 | 已实现 |
| /cancel | 取消当前 run | 已实现 |
| /status | 查看 provider/model/running/tools/config | 已实现 |
| /provider | 切换 provider | 已实现 |
| /model | 切换 model | 已实现 |
| /models | 查看当前 provider 模型 | 已实现 |
| /config show/set/save | 查看、临时修改、保存配置 | 已实现 |
| /tools | 查看启用工具 | 已实现 |
| /sessions | 列出已保存会话 | 已实现 |
| /compact | 触发上下文压缩 | 已实现（stub） |
| /agent | 切换/查看 agent profile | 未实现 |
| /permissions | 查看/编辑工具权限规则 | 未实现 |
| /approve / /deny | 响应挂起的工具审批 | 未实现（但 TUI 有 approve_pending/deny_pending） |
| /memory | 查看/写入记忆 | 未实现 |
| /mcp | 管理 MCP server | 未实现 |
| /web | 管理 web search provider | 未实现 |

## 8. 自定义 Agent Profile

每个 profile 至少包含：

- name / description
- system_prompt
- model / provider
- tools
- permissions
- context_policy
- working_directory

示例 profiles：

| Profile | 工具组合 | 权限策略 |
|---|---|---|
| reader | list_files、read_file、grep、web_search | 全部 auto，只读 |
| coder | read_file、grep、write_file、apply_patch、shell | 读 auto，写/shell ask |
| docs | read_file、grep、write_file、web_search | docs 路径 ask，网络 auto |
| ci-fixer | read_file、grep、apply_patch、shell:cargo test | cargo test auto，写 ask |
| researcher | web_search、fetch_url、read_file、memory_write | search auto，memory write ask |

## 9. 阶段路线图与进度

### P0：精简内核 + 可配置工具/权限骨架

状态：已完成

- AgentBuilder / ToolSet / PermissionPolicy 友好 API。
- ToolDecision::Ask + SharedApprovalChannel + TUI 审批通道。
- RulePattern 匹配：工具名、分类、路径前缀、命令前缀。
- 工具权限配置模型与 TOML 解析（AgentProfileConfig、PermissionRuleConfig）。
- 已测试通过。

验收标准达成：

- 可以通过 Rust API 创建只读 agent（ToolSet::new().read().grep()）。
- 同一工具在不同 agent profile 下可有不同审批策略。
- 未批准的 mutating 工具不会执行，模型收到结构化拒绝错误。

### P1：编码 agent 必需工具

状态：已完成

- write_file：workspace-scoped 写入保护。
- edit_file：字符串首次匹配替换。
- apply_patch：unified diff 解析、hunk 匹配、行号/全文 fallback。
- shell：sh -c 执行、analyse_command_safety（命令链检测、$(demo)/反引号/LD_PRELOAD 路径逃逸）。
- workspace-scoped 写入保护：resolve_read/resolve_write 双重校验。
- TUI approve/deny 交互。
- 已测试通过。

验收标准达成：

- 默认配置下读工具自动执行，写入/Shell 向用户请求批准。
- TUI 展示工具参数摘要，通过 approve_pending/deny_pending 响应。
- cargo test、git status 等命令可通过 ShellTool::is_trusted_command 自动批准。

### P2：Web/Search 和网络工具

状态：部分完成

- WebSearch trait + DuckDuckGoSearch 后端（免 API key，HTML 解析）。
- WebSearchTool 工具封装。
- fetch_url 工具（私有 IP 阻断、HTML 纯文本提取）。
- ToolCategory::Network 权限分类。
- 待实现：多搜索后端注册、搜索结果 citation/schema 标准化、TUI /web slash command。

### P3：会话、上下文压缩、记忆

状态：部分完成

- SessionStore trait + JsonlSessionStore（save/load/list/delete，JSONL 文件后端）。
- /sessions slash command（列出会话）。
- /compact slash command（触发生成摘要）。
- SummaryContextBuilder（摘要压缩上下文构建器）。
- memory_read / memory_write 工具（.joker-memory/ 文件存储）。
- 待实现：TUI 会话恢复、/compact 模型驱动摘要管道、/memory slash command。

### P4：扩展生态

状态：未开始

- MCP client：发现外部工具并映射到 Joker Tool。
- MCP server：让 Joker 暴露自身工具。
- Hook 系统：run/tool/model/session 生命周期事件。
- Skills/插件目录：加载额外 prompt、工具、权限规则。
- HTTP/ACP 宿主。
- /mcp slash command。

## 10. 架构边界

| Crate | 职责 | 调整方向 |
|---|---|---|
| joker | 纯 agent kernel：trait、协议、loop、策略、审批 | 保持精简，继续丰富 builder/permission/approval 抽象 |
| joker-tools | 内置工具实现 | 已拆分 readonly、mutating、shell、network、memory |
| joker-provider | Provider 适配 | 输出统一 ModelResponseEvent |
| joker-config | 文件配置到 runtime/profile 解析 | 已含 AgentProfileConfig、PermissionRuleConfig |
| joker-tui | 宿主实现 | 增加审批 UI、approve/deny 绑定、会话管理 |

原则：

- 核心 crate 不依赖 TUI、HTTP、具体 Provider。
- 工具实现不直接询问用户，询问必须通过 policy/host 完成。
- 权限策略不绑定 TUI，CLI/HTTP 也能复用。
- 配置是 API 的投影，不能替代 Rust API 的可组合性。

## 11. 当前最高优先级

1. 完善 P2：多搜索后端注册、搜索结果标准化、/web slash command。
2. 完善 P3：TUI 会话恢复功能、/compact 模型驱动摘要、/memory slash command。
3. 开始 P4：MCP client 集成。
4. 补充 P1/P2/P3 各模块的测试覆盖率（特别是 shell、apply_patch、fetch_url 的边界情况）。
