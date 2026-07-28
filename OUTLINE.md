# Joker 项目总纲

> 阅读顺序：第 1-8 章保留早期产品蓝图和参考项目调研，第 9-10 章是基于当前仓库源码的实态调研补充。后续实现、拆任务、修 bug 时，优先以第 9-10 章记录的“已实现边界 / 当前缺口 / 下一步路线”为准；第 1-8 章中与代码现状冲突的说法应视为历史规划，而不是当前事实。

## 1. 项目定位

Joker 是一个 API 封装优先、高度自定义化的 Rust 编码代理。它是一个 TUI 交互的 Rust 编码代理：

- **模型 Provider**：当前先只支持 DeepSeek 的 API 接入（OpenAI 兼容协议 `/v1/chat/completions`，参考 DeepSeek-Reasonix 的 `provider/openai` 实现）。后续通过统一的 Protocol/Route 抽象（参考 OpenCode 的 `Protocol<Body, Frame, Event, State>` 四轴分解：Protocol + Endpoint + Auth + Framing）扩展到其他厂商。
- **工具集合**：`read`、`write`、`edit`、`apply_patch`、`shell`、`grep`、`glob`、`web_search`、`web_fetch`、`memory_read`、`memory_write`、`todo_write`、MCP 工具等。工具分两层：核心协议层定义 `Tool` trait（参考 CodeWhale 的 `ToolHandler` + Codex 的 `ToolExecutor`），应用层实现具体逻辑（参考 CodeWhale 的 `ToolSpec` trait）。
- **工具权限**：每个 Agent 的每个工具独立设置 3 种权限——`ask`（调用前询问）、`auto-accept`（直接调用）、`disabled`（禁止调用）。参考 CodeWhale 的 `ExecPolicyEngine` 分层规则集（BuiltinDefault → Agent → User）和 OpenCode 的通配符匹配 + last-match-wins 策略。
- **上下文构建**：原始对话 + 固定窗口 + 摘要压缩（参考 claude-code 的多策略压缩：snip/micro/cached/auto/reactive）+ 项目上下文（参考 DeepSeek-Reasonix 的 REASONIX.md 层级记忆）+ 规则/记忆注入（参考 claude-code 的 MEMORY.md 索引 + 独立 memory 文件）。
- **运行宿主**：库 API → TUI → CLI one-shot，后续可扩展到 HTTP/ACP/MCP server（参考 codex 的 SQ/EQ 模式：Submission Queue + Event Queue 将前端与核心解耦）。

## 2. 总体规划

每次工作前需要认真调研 `../agents/` 中各个参考项目对应部分的实现。

### 2.1 核心 loop 必须小而稳定

成熟项目都把模型流、工具调用、权限拦截、事件输出作为稳定主轴，复杂功能尽量挂在外围。

**参考实现要点：**

- **OpenCode**：`SessionRunner.run()` → `runTurn()` 双层循环。内层循环处理 provider 流式响应 → 工具调用 → 结果反馈 → 判断是否继续；外层循环处理 steer/queue 中途注入。事件通过 `EventV2.publish()` 持久化，消息通过 projector 投影到 `session_message` 表。整个生命周期由 `SessionExecution.wake()` / `resume()` / `interrupt()` 管理。
- **CodeWhale**：`engine.rs` 通过 `mpsc` 通道接收 `Op`（操作），发射 `Event` 到 UI。`turn_loop.rs` 处理流式响应解析 → 工具规划（`dispatch.rs`）→ 权限检查（`approval.rs`）→ 工具执行（`tool_execution.rs`）→ 结果反馈 → 循环判断。
- **codex**：`Session::spawn()` 创建 `submission_loop`（在 `rx_sub` 上循环），`Turn::run_turn()` 驱动单次 LLM 调用 → 流式处理 → 工具执行循环。`TurnState` 跟踪所有进行中的通道（动态工具、挂起审批、挂起权限、用户输入）。
- **pi**：双层循环 —— 内层 `hasMoreToolCalls` 循环（流式响应 → 工具执行 → 循环），外层 `follow-up` 循环（检查后续消息，重新进入内层）。`Agent` 类管理 `steer()` 和 `followUp()` 队列，防止并发执行。
- **DeepSeek-Reasonix**：`Controller`（与前端无关的会话驱动器）→ `Agent.Run()` 主循环 → `a.stream()` 流式调用。中间支持 stream reconnect（最多 3 次，仅在零输出时重放）。

**Joker 核心 loop 设计细化：**

```
┌─ JokerRuntime::spawn(session) ─────────────────────────────┐
│  ┌─ OpLoop (rx_op) ──────────────────────────────────────┐ │
│  │  Op::SendMessage → turn()                             │ │
│  │  Op::Cancel → abort()                                 │ │
│  │  Op::Interrupt → interrupt()                          │ │
│  │  Op::Approve → resolve_approval()                     │ │
│  │  Op::Compact → compact()                              │ │
│  │  Op::SwitchAgent → switch_agent()                     │ │
│  │  Op::Shutdown → shutdown()                            │ │
│  └───────────────────────────────────────────────────────┘ │
│                                                             │
│  ┌─ turn(session, input) ────────────────────────────────┐ │
│  │  loop:                                                 │ │
│  │    1. build_context()    — 组装消息历史 + 系统提示     │ │
│  │    2. resolve_model()    — 选择模型                    │ │
│  │    3. materialize_tools() — 按 agent 权限过滤工具      │ │
│  │    4. compact_if_needed() — 上下文压缩检查             │ │
│  │    5. stream(payload)    — 调用 provider，发射 Event   │ │
│  │    6. for tool_call in tool_calls:                     │ │
│  │         check_permission() → approve() → execute()     │ │
│  │    7. if has_tool_results → loop                       │ │
│  │    8. check_steer() — 检查中途注入                     │ │
│  └───────────────────────────────────────────────────────┘ │
│  ┌─ EventSink (tx_event) ───────────────────────────────┐ │
│  │  TurnStarted, TextDelta, ReasoningDelta,              │ │
│  │  ToolDispatch, ToolResult, ToolProgress,              │ │
│  │  ApprovalRequest, CompactionStarted,                  │ │
│  │  TurnDone, Error, Retrying, Usage                     │ │
│  └───────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

关键设计原则：
- **Op/Event 分离**：前端通过 `Op` 队列向核心发送命令，核心通过 `EventSink` 向外发射事件（参考 codex 的 SQ/EQ 模式 + DeepSeek-Reasonix 的 `control.Controller`）。
- **单会话单运行**：通过 `RunState` 防止同一会话并发执行（参考 MiMo-Code 的 `Effect.Runner` + BusyError）。
- **双层循环**：内层处理工具调用往返，外层处理 steer/follow-up 中途注入（参考 pi 的双层循环设计）。
- **流式重连**：仅在尚未发送任何输出时重放请求（参考 DeepSeek-Reasonix 的 `streamWithReconnect`）。
- **loop 容错**：区分可恢复错误（rate limit → 重试 + 退避）和不可恢复错误（auth failure → 终止），参考 gemini-cli 的 `GeminiChat` 重试逻辑（最多 4 次尝试，指数退避）。

### 2.2 工具和权限必须分离

工具只声明能力与风险，策略层决定是否允许执行。

**本项目特色流程：**

```
运行 joker → 进入 Joker TUI → 进入 /agent →
  ├─ 已有内置 agent（plan / build / yolo）
  └─ [Add a new agent] 选项（自定义高度定制化 agent）
       └─ 进入 Add a new agent →
            1. 命名新的 agent（用户输入）
            2. 对每个工具独立设置权限：
               ├─ ask（调用前询问）
               ├─ auto-accept（直接调用，不询问）
               └─ disabled（禁止调用）
            3. 自动生成 "{agent名称}_agent.md" 独立约束文件
```

**内置 3 个 agent**（严格参考 OpenCode、CodeWhale 的实现方式全量实现，搭配本项目的工具自定义权限）：

| Agent | 定位 | 工具权限 | 参考来源 |
|-------|------|---------|---------|
| **plan** | 只读分析，输出计划 | read/glob/grep/web_search: auto-accept；write/edit/shell/apply_patch: disabled | OpenCode 的 plan mode（hardPermission deny edit）+ CodeWhale 的 Explore/Plan 角色（PermissionSet::read_only, ShellPolicy::None） |
| **build** | 完整编码能力 | 全部工具: ask（默认询问） | OpenCode 的默认 `build` agent + CodeWhale 的 Implementer 角色（PermissionSet::full, ShellPolicy::Full） |
| **yolo** | 自动执行，无需询问 | read/write/edit/shell/apply_patch/grep/glob: auto-accept；web_search/web_fetch: ask | CodeWhale 的 auto_approve 模式 + gemini-cli 的 YOLO ApprovalMode |

**参考实现要点（工具层）：**

- **CodeWhale 双层工具系统**（适合 Rust 实现）：
  - 协议层：`ToolHandler` trait（`kind()`, `is_mutating()`, `handle(invocation) → ToolOutput`）+ `ToolRegistry` 管理 `Arc<dyn ToolHandler>`
  - TUI 层：`ToolSpec` trait（`name()`, `description()`, `input_schema()`, `capabilities() → Vec<ToolCapability>`, `approval_requirement()`, `is_read_only()`, `supports_parallel()`, `execute()`）
  - `ToolCapability` 枚举：`ReadOnly`, `WritesFiles`, `ExecutesCode`, `Network`, `Sandboxable`, `RequiresApproval`
  - `ApprovalRequirement` 枚举：`Auto`（不询问）, `Suggest`（建议但可跳过）, `Required`（必须询问）

- **codex 工具执行编排**：`ToolOrchestrator` 集中处理 审批 → 沙箱选择 → 执行 → 沙箱升级重试（审批结果缓存，无需重复审批）

- **OpenCode 工具注册**：两层注册 —— `ApplicationTools`（全局静态）+ `Tools.register()`（Scope 内注册，自动清理）。`materialize(permissions)` 按 agent 权限过滤工具列表。

- **gemini-cli 调度器**：事件驱动状态机 —— `Validating → Scheduled → Executing → Success | Error | Cancelled`，支持并行批处理 + 尾部调用链。

**参考实现要点（权限层）：**

- **OpenCode 权限评估**：通配符匹配 + `findLast`（last-match-wins），默认 `ask`。`PermissionSaved` 表持久化 "always allow" 决策。Agent 有 `permissions: Ruleset` 字段。

- **CodeWhale ExecPolicyEngine**：三层优先级规则集 —— `BuiltinDefault(0) → Agent(1) → User(2)`。`deny` 始终优先，链式命令从不自动信任。`PermissionAction` 枚举：`Allow`, `Ask`, `Deny`。

- **MiMo-Code 三层权限合并**：`agent.permission + user/session rules + hardPermission`（hardPermission 最后追加，不可被配置覆盖）。Plan mode 用 hardPermission 实现 "deny all edits except plan files"。

- **gemini-cli PolicyEngine**：优先级排序的规则链，支持 shell 命令解析（处理重定向、包装器、管道），对复合命令逐段评估。

**Joker 权限系统设计细化：**

```
ToolDefinition {
    name, description, input_schema,
    capabilities: Vec<ToolCapability>,  // ReadOnly | WritesFiles | ExecutesCode | Network
    default_approval: ApprovalRequirement,  // Auto | Suggest | Required
}

AgentPermission {
    agent_name: String,
    tool_permissions: HashMap<ToolName, PermissionSetting>,  // Ask | AutoAccept | Disabled
    constraint_file: PathBuf,  // "{agent_name}_agent.md"
}

PermissionEngine::evaluate(tool, agent, action, resource) → Decision {
    // 1. agent 级 disabled 检查 → 如果是 Disabled → Deny
    // 2. agent 级 auto-accept 检查 → 如果是 AutoAccept → Allow
    // 3. agent 级 ask → 进入交互式审批流程
    //    交互式审批提供选项：Allow Once | Allow for Session | Deny
    // 4. 默认回退 → Ask
}
```

### 2.3 Provider 适配层不能污染内核

Provider 配置当前只允许单一入口：

```
运行 joker → 进入 Joker TUI → 进入 /provider →
  → 选择模型供应商（当前 DeepSeek）
  → 填入用户自己的 API key
  → 通过用户的 API key 自动识别模型并加入模型列表
  → 进入 /model 后自动加载可用模型
```

**这里需要把 OpenCode 的全量实现策略照搬并翻译为 Rust 实现，代码量大一点也没关系。注意模型列表应该根据各个供应商的URL与API自动识别并做好路由而非预设！**

**参考实现要点：**

- **OpenCode 的 Protocol/Route 分解**（最值得照搬的架构）：
  ```
  Protocol<Body, Frame, Event, State>  // API 语义契约
    ├─ body: { schema, from(request) → Body }
    ├─ stream: { event schema, initial(state), step(state, event) → [LLMEvent], terminal?(event) }
    └─ onHalt?(state) → [LLMEvent]

  Route<Body, Prepared>  // 组合 Protocol + 部署关注点
    ├─ id, provider, protocol, endpoint, auth, transport, defaults
    ├─ with(patch) → Route  // 派生覆盖
    ├─ model(input) → Model
    ├─ prepareTransport(body, request) → Prepared
    └─ streamPrepared(prep, request, runtime) → Stream<LLMEvent>
  ```
  四个正交部署轴：**Protocol**（API 形状）+ **Endpoint**（URL）+ **Auth**（凭证策略）+ **Framing**（SSE/WS/AWS event stream）。

- **CodeWhale 的 Provider 体系**：
  - `WireFormat` 枚举：`ChatCompletions`（OpenAI 兼容）、`Responses`（OpenAI Responses API）、`AnthropicMessages`
  - `ProviderKind` 枚举覆盖 30+ 厂商
  - `ModelRegistry`：~100 个 `ModelInfo` 条目，`resolve()` 实现回退链：请求模型 → provider 提示 → 别名映射 → provider 默认 → 全局回退
  - `HarnessPosture`：按 provider/model 匹配预设策略（Standard / CacheHeavy / Lean）

- **DeepSeek-Reasonix 的 DeepSeek 特定处理**（直接参考）：
  - 基于 host 的供应商检测：`IsDeepSeek(api.deepseek.com)`、`IsMiniMax`、`IsZhipu` 等
  - 各后端 thinking/reasoning 协议差异处理：DeepSeek 用 `thinking.type=enabled` + `reasoning_effort`，MiniMax 用 `<think>` 标签
  - `CanonicalizeSchema`：递归规范化 JSON Schema，保证工具定义跨 MCP 重启字节一致
  - `NormalizeMessages`：快路径零拷贝直传 + 慢路径修复（配对 tool_calls/tool_results、回填空位、修复截断 JSON）
  - Stream reconnect：仅在零模型输出时重放请求，防止重复输出

- **pi 的凭证存储**：
  - `CredentialStore` 接口（按 provider ID 键控）+ `AuthStorage`（持久化到 `~/.pi/auth.json`，权限 `0o600`，文件锁保护读写）
  - API key 解析链：运行时覆盖 → 存储的 api_key → OAuth 令牌（自动刷新）→ 环境变量
  - OAuth 刷新采用双重检查锁定 + 跨进程重试

- **gemini-cli 的模型路由**：
  - `CompositeStrategy`（责任链）：`[Fallback → Override → ApprovalMode → Classifier → Default]`
  - 非终端策略静默失败（返回 null），终端策略保证决策

**Joker Provider 层设计细化：**

```
ProviderRegistry {
    providers: HashMap<ProviderKind, ProviderSpec>,
    model_catalog: Vec<ModelInfo>,
}

ProviderSpec {
    kind: ProviderKind,        // DeepSeek(当前), OpenAI, Anthropic, ...
    wire_format: WireFormat,   // ChatCompletions | AnthropicMessages | Responses
    default_base_url: Url,
    auth: AuthConfig,          // ApiKey(env_var, stored) | OAuth
    thinking_style: ThinkingStyle,  // ThinkingType | EnableThinking | ReasoningSplit | ThinkTags
    supports_prompt_caching: bool,
    supports_stream_options: bool,
}

ModelInfo {
    id: String,
    provider: ProviderKind,
    context_window: usize,
    max_output_tokens: usize,
    capabilities: ModelCapabilities,  // tools, vision, streaming, reasoning
    aliases: Vec<String>,
}

// 凭证存储（参考 pi 的 AuthStorage）
CredentialStore {
    // 持久化到 ~/.joker/auth.json (0o600 权限)
    // 解析链: runtime_override → stored → env_var
    get_api_key(provider: ProviderKind) → Option<String>
    set_api_key(provider: ProviderKind, key: String)
    delete_api_key(provider: ProviderKind)
}

// 模型解析链（参考 CodeWhale 的 ModelRegistry::resolve）
ModelResolver::resolve(requested, provider_hint) → ModelInfo {
    requested_model
        → alias_map.lookup()
        → provider_default
        → global_fallback
}
```

### 2.4 会话和事件是后续扩展基础

OpenCode / Codex / pi 都把 session、thread store、事件流作为恢复、压缩、UI、协议集成的基础。

**参考实现要点：**

- **OpenCode 事件溯源架构**（最完整的参考）：
  - 所有会话状态变更都是持久事件（`event_sequence` + `event` 表，序列号保证 exactly-once）
  - 消息是事件的读模型投影（projector 在事件提交事务内原子写入 `session_message` 表）
  - ~30+ 事件类型：`PromptAdmitted`, `Prompted`, `Step.Started/Ended`, `Text.Started/Delta/Ended`, `Tool.Called/Success/Failed`, `Reasoning.Started/Delta/Ended`, `Compaction.*`, `AgentSwitched`, `ModelSwitched`

- **pi 的会话树**（适合本地优先存储）：
  - 不可变追加日志 JSONL 格式（第 1 行 header + 后续行 `SessionTreeEntry`）
  - 树形结构：每个条目有 `id`、`parentId`，`LeafEntry` 标记当前位置
  - `getPathToRoot(leafId)` 重建消息分支，`navigateTree()` 切换分支
  - `Fork` 通过在共享父条目处分叉创建新 JSONL 文件

- **CodeWhale 的 SQLite 存储**：
  - `threads` 表（会话元数据 + `current_leaf_id`）+ `messages` 表（树形结构，`parent_entry_id`，递归 CTE 遍历）
  - `checkpoints` 表（命名状态快照 JSON blobs）
  - 追加式 JSONL 会话索引（5000 行自动压缩）

- **claude-code 的会话持久化**：
  - JSONL 文件存储在 `~/.claude/projects/<cwd_hash>/sessions/`
  - 双层存储：内存环形缓冲区（最近 1000 条）+ 文件 JSONL
  - 批量写入（100ms 延迟 JSON.stringify）减少 I/O

- **codex 的会话生命周期**：
  - `SessionConfiguration`（不可变配置）→ `Session::new()` → `Session::spawn()`（创建通道 + 启动 `submission_loop`）
  - `RolloutRecorder`：将 `RolloutItem` 追加到 rollout 文件
  - `LiveThread` + `ThreadStore` trait 抽象持久化

**Joker 会话/事件系统设计细化：**

```
// 事件类型（参考 OpenCode + codex + DeepSeek-Reasonix）
Event {
    TurnStarted {
        session_id, turn_id, agent_name, model_id,
    },
    TextDelta { delta, turn_id },
    ReasoningDelta { delta, turn_id },
    ToolDispatch { call_id, tool_name, args_preview },
    ToolResult { call_id, output_summary, diff_stats },
    ToolProgress { call_id, partial_output },
    ApprovalRequest { call_id, tool_name, resources, risk },
    Usage { input_tokens, output_tokens, cache_hit_tokens },
    TurnDone { turn_id, stop_reason },
    CompactionStarted { trigger, current_tokens, threshold },
    CompactionDone { tokens_before, tokens_after },
    AgentSwitched { from, to },
    ModelSwitched { from, to },
    Error { kind, message, recoverable },
    Retrying { attempt, max_attempts, reason },
}

// 会话存储（参考 pi 的 JSONL 树 + CodeWhale 的 SQLite）
SessionStore {
    // 主存储：SQLite
    // - sessions 表：id, cwd, created_at, updated_at, current_agent
    // - messages 表：id, session_id, parent_id, role, content, tool_calls, timestamp
    //   （树形结构，递归 CTE 查询路径）
    // - checkpoints 表：id, session_id, label, snapshot_json
    //
    // 辅助索引：追加式 JSONL（快速扫描最近会话）+ 5000 行自动压缩
}

// 上下文压缩（参考 claude-code 多策略 + OpenCode 的 LLM 压缩）
CompactionStrategy {
    // soft: 仅通知，不压缩（上下文 50% 时）
    // compact: LLM 摘要压缩（上下文 80% 时，保留最近 8K tokens + 摘要）
    // force: 激进截断（上下文 90% 时，仅保留系统提示 + 最近消息）
    // micro: 相同文件读取替换为 stub（不调用 LLM）
    trigger_ratio: (soft: 0.5, compact: 0.8, force: 0.9),
    keep_recent_tokens: 8000,
}
```

### 2.5 扩展点比内置功能更重要

MCP、插件、skills、hooks 的价值在于让用户自定义 agent，而不是把所有能力写死在主程序里。

**参考实现要点：**

- **claude-code 的 Skills 系统**：Skills 就是 prompt-type 命令，从 5+ 来源加载（bundled skills → builtin plugin → skill directory → workflow commands → plugin commands），按 file path pattern 门控可见性。
- **codex 的 Skills 系统**：模块化指令包，作用域 `User | Repo | System | Admin`，通过 `AGENTS.md` 配置，`@<skill-name>` 显式请求，命令内容自动检测隐式调用。
- **gemini-cli 的命令加载器**：`ICommandLoader` 接口 → `BuiltinCommandLoader` + `FileCommandLoader` + `SkillCommandLoader` + `McpPromptLoader`，通过 `CommandService` 并行加载，`SlashCommandResolver` 优先级解决名称冲突。
- **OpenCode 的 Plugin 系统**：`Plugin.Service` 提供事件钩子（`chat.system.transform`, `chat.messages.transform`, `chat.tools.transform`），tool 的 shell 解析可由插件自定义。
- **MiMo-Code 的 QuickJS 沙箱**：GPT 模型用 `exec` 工具在 QuickJS 沙箱中运行 TypeScript，两层安全（QuickJS 隔离 + host 工具权限检查），资源限制（50 次工具调用、60s 活跃计算、64 MiB 内存）。

**Joker 扩展系统设计细化：**

```
// MCP 集成（参考 claude-code 的 MCP 一等公民 + OpenCode 的 MCP 工具发现）
McpRuntime {
    // 传输: stdio (子进程) | sse (HTTP SSE) | http (Streamable HTTP)
    // 工具命名: mcp__<server_name>__<tool_name>
    // 生命周期: 连接 → 工具发现 → 工具注册 → 断线检测 → 重连
    // 工具与内置工具共用同一个 Tool trait，权限检查无差别
}

// Hook 系统（参考 gemini-cli + claude-code）
Hooks {
    // 会话生命周期: on_session_start, on_session_end
    // 回合生命周期: on_turn_start, on_turn_end
    // 工具生命周期: before_tool_call (可阻止), after_tool_call (可修改结果)
    // 消息生命周期: before_provider_request, after_provider_response
    // 事件钩子: on_compaction, on_error, on_approval
}

// Skill 系统（参考 claude-code + codex）
Skill {
    name, description, paths (文件路径门控),
    prompt_content (注入到系统提示),
    allowed_tools (可选，限制工具范围),
}
```

## 3. 核心 API 设计方向

Joker 的核心设计目的是 API 优先，每一个工具都包装为功能完整、接口单一的 API，方便工具与权限的组合自由性以及新建自定义 agent 的结构化。

**参考实现要点：**

- **OpenCode 的 `Tool.make()`**：每个工具是 opaque object，运行时通过 `WeakMap` 存储元数据。`Tool.withPermission(tool, "edit")` 附加权限标签，`Tool.definition(name, tool)` 惰性构建 `ToolDefinition`，`Tool.settle(tool, call, context)` 验证 + 执行 + 编码输出。
- **CodeWhale 的 `ToolSpec` trait**：声明式接口 —— `name()`, `description()`, `input_schema()`, `capabilities()`, `approval_requirement()`, `execute()`。每个工具只声明能力和风险，不直接处理审批。
- **codex 的 dual-plane 设计**：`ToolSpec`（模式/描述，给模型看）+ `ToolExecutor`（运行时执行，独立可插拔）。`ToolOrchestrator` 集中编排审批 + 沙箱 + 重试。
- **pi 的三阶段工具执行**：prepare（验证参数 + beforeToolCall 钩子）→ execute（运行工具 + 收集部分更新）→ finalize（afterToolCall 钩子 + terminate 标志）。

**Joker 工具 API 设计细化：**

```rust
// 每个工具对外暴露为功能完整、接口单一的 API 模块
// 例如 read 工具：
pub mod tool_read {
    pub struct ReadTool;
    impl Tool for ReadTool { ... }
    // 公开的纯 API，可直接被库调用者使用
    pub async fn read_file(path: &Path, offset: Option<usize>, limit: Option<usize>) -> Result<String> { ... }
}

pub mod tool_write { ... }
pub mod tool_edit { ... }
pub mod tool_shell { ... }
pub mod tool_grep { ... }
pub mod tool_glob { ... }
pub mod tool_apply_patch { ... }
pub mod tool_web_search { ... }
pub mod tool_web_fetch { ... }
pub mod tool_memory_read { ... }
pub mod tool_memory_write { ... }
pub mod tool_todo_write { ... }
```

这样无论是内置 agent 调用、TUI 交互、还是用户自定义 agent，都通过相同的 API 入口，权限检查和工具执行完全解耦。

## 5. 工具系统规划

工具的职责是声明能力、输入 schema、输出格式和风险元数据，不直接处理用户审批。

**工具清单：**
`list_files`、`read_file`、`grep`、`write_file`、`edit_file`、`apply_patch`、`shell`、`web_search`、`fetch_url`、`memory_read`、`memory_write`、`todo_write`、`mcp`

这些工具中大部分已经在当前项目代码中实现，但现在需要**完全照搬 `../agents/` 中各个参考项目中的最优实现替换本项目中原本的实现**，代码量大一点也没关系。

**各工具最优实现参考：**

| 工具 | 最优参考 | 关键实现细节 |
|------|---------|-------------|
| `read_file` | CodeWhale + pi | 支持行偏移/limit、代码围栏输出、图片读取、BOM 保留、换行符检测（LF/CRLF） |
| `write_file` | OpenCode + pi | BOM 保留、换行符检测、写入前读取验证、截断保护 |
| `edit_file` | OpenCode | `old_string` + `new_string` + `replace_all` 语义、精确字符串匹配、出现次数校验（0次报错，>1次需 replaceAll）、换行符自适应、stale-file 检测（writeIfUnchanged） |
| `apply_patch` | OpenCode | 完整 patch 文本解析（add/update/delete hunks）、顺序执行、部分失败报告、diff 输出 |
| `shell` | CodeWhale + DeepSeek-Reasonix | 输出环形缓冲区 + 文件后备、节流更新（100ms）、进程树终止、超时控制、沙箱支持、后台执行（bg job）+ evidence leasing |
| `grep` | claude-code + CodeWhale | ripgrep/ugrep 集成、行号、上下文行、文件类型过滤 |
| `glob` / `list_files` | CodeWhale | 模式匹配、递归深度控制、`.gitignore` 感知 |
| `web_search` | gemini-cli + DeepTutor | 搜索引擎 API 封装、结果结构化、来源标注 |
| `web_fetch` / `fetch_url` | claude-code + DeepTutor | HTTP 客户端、内容提取（HTML→Markdown）、大小限制、超时 |
| `memory_read/write` | claude-code + DeepSeek-Reasonix | 层级记忆文件（MEMORY.md 索引 + `memory/` 目录独立文件）、frontmatter 格式、`@path` 引用导入、`#<note>` 快速追加 |
| `todo_write` | OpenCode + CodeWhale | 合并更新（不覆盖已有状态）、进度追踪、完成步骤门控 |
| `mcp` | claude-code + OpenCode | MCP 工具动态包装为 Tool trait、命名规范化（`mcp__server__tool`）、工具发现与重连 |

**工具元数据设计（综合 CodeWhale + gemini-cli + codex）：**

```rust
struct ToolMetadata {
    name: String,
    description: String,
    input_schema: JsonSchema,

    // 风险元数据（不直接决定审批，仅供策略层参考）
    capabilities: Vec<ToolCapability>,  // ReadOnly | WritesFiles | ExecutesCode | Network | Sandboxable
    default_approval: ApprovalRequirement,  // Auto | Suggest | Required

    // 执行模型
    is_read_only: bool,
    supports_parallel: bool,     // 是否可与其他工具并行执行
    is_concurrency_safe: bool,   // 是否可在多个 turn 间并发安全

    // 可用性
    defer_loading: bool,         // 延迟加载（MCP 工具用）
    model_visible: bool,         // 是否对模型可见

    // 权限匹配
    permission_key: String,      // 策略匹配键，如 "shell:git status"、"mcp:github_create_issue"
    resource_pattern: String,    // 操作资源模式，用于通配符匹配

    // 展示
    display_name: String,        // TUI 审批弹窗展示名
    display_summary: fn(args) -> String,  // 从参数生成摘要
}
```

## 6. 权限系统规划

权限系统是三层结构：

| 层级 | 含义 | 触发时机 |
|------|------|---------|
| `ask` | 调用时询问 | 该 agent 使用该工具时弹出对话框 |
| `auto-accept` | 默认允许 | 该 agent 使用该工具时直接放行 |
| `disabled` | 禁止调用 | 该 agent 不能使用该工具（工具列表里直接隐藏） |

**交互式审批流程（ask 时）：**

参考 CodeWhale 的 `approval.rs` + claude-code 的 `handleInteractivePermission`：

```
工具调用 → 检查 agent 权限:
  ├─ disabled → 拒绝，返回错误给模型（工具不出现在工具列表中）
  ├─ auto-accept → 直接执行
  └─ ask → TUI 弹出审批对话框:
       ├─ [A] Allow Once — 本次调用允许
       ├─ [S] Allow for Session — 本次会话内该工具都允许（会话级缓存）
       └─ [D] Deny — 本次调用拒绝，返回拒绝原因给模型
```

**权限评估参考：**

- **OpenCode**：`evaluate(action, resource, ...rulesets) → Rule`，通配符匹配 + `findLast`，默认 `ask`。`PermissionSaved` 表持久化 "always" 决定。
- **CodeWhale ExecPolicyEngine**：三层优先级 `BuiltinDefault(0) → Agent(1) → User(2)`，deny 始终优先，链式命令分解逐段评估。
- **MiMo-Code hardPermission**：最后追加，不可被配置覆盖，用于实现不可绕过的安全策略。
- **gemini-cli**：优先级排序规则链，shell 重定向自动降级 `Allow → Ask`（YOLO 模式除外），`ALWAYS_ALLOW_PRIORITY_FRACTION = 950` 区分 "始终允许" 规则。

**Joker 权限引擎设计：**

```rust
struct PermissionEngine {
    // 每 agent 的工具权限配置
    agent_permissions: HashMap<AgentName, AgentPermission>,
    // 会话级临时权限（Allow for Session）
    session_grants: HashMap<(SessionId, ToolName), GrantExpiry>,
}

enum PermissionDecision {
    Allow,                          // auto-accept 或 session 缓存命中
    Ask { tool: String, resources: Vec<String>, risk: RiskLevel },
    Deny { reason: String },
}

impl PermissionEngine {
    fn evaluate(&self, agent: &AgentName, tool: &ToolName, args: &Value) → PermissionDecision {
        // 1. 查 agent_permissions[agent][tool]
        //    - Disabled → Deny
        //    - AutoAccept → Allow
        //    - Ask → 查 session_grants
        // 2. 查 session_grants
        //    - 命中且未过期 → Allow
        //    - 未命中 → Ask（进入交互式审批）
    }
}
```

**安全原则（综合各项目）：**
- deny 始终优先（CodeWhale 原则）
- 链式 shell 命令从不自动信任（CodeWhale 原则）
- shell 重定向（`>`、`|`、`$(...)`）自动降级 `Allow → Ask`（gemini-cli 原则）
- hardPermission 机制为内置 agent 保留不可绕过的安全底线（MiMo-Code 原则）

## 7. Slash Command 规划

当前先只支持以下 command，多余的先清理掉：

```
/exit      — 退出 Joker TUI
/provider  — 配置模型供应商 + API key
/model     — 查看/切换可用模型
/sessions  — 查看/恢复历史会话
/compact   — 手动触发上下文压缩
/agent     — 查看/切换/新建 agent
```

**slash command 必须支持自动补全最优匹配项与跳转**，这里要完全照搬 `../agents/` 中各个参考项目中的最优实现替换本项目中原本的实现。slash command 是提升用户体验的关键一步。

**参考实现要点：**

- **claude-code 的命令系统**：三种类型 —— `prompt`（展开为模型输入）、`local`（本地执行返回文本）、`local-jsx`（渲染 React UI）。命令从 5+ 来源加载：bundled skills → builtin plugin → skill directory → workflow → plugins。`getCommands()` 按 cwd 记忆化，auth/provider 可用性过滤。

- **CodeWhale 的命令注册表**：`Command` trait（`info() → &CommandInfo` + `execute(app, args) → CommandResult`），`CommandInfo` 包含 `name`, `aliases`, `usage`, `description_id`。9 个命令组（Core, Session, Config, Debug, Project, Skills, Memory, Plugins, Utility）。按发现级别组织：`Primary`（根级可见）+ `Advanced`（带前缀）+ `Compatibility`（隐藏）。

- **gemini-cli 的 CommandService**：`ICommandLoader` 接口 → 多个加载器并行加载 → `SlashCommandResolver` 解决名称冲突（非内置命令加来源前缀：`user.name`、`workspace.name`）。`SlashCommand` 接口支持 `completion()` 方法（返回补全候选项）和 `subCommands`（子命令）。

- **OpenCode 的命令定义**：`Command.Info = { name, template, model?, agent?, attachments? }`，State-based mutable store 管理注册。

**Joker Slash Command 设计细化：**

```rust
// 命令注册（参考 CodeWhale 的 Command trait）
trait SlashCommand: Send + Sync {
    fn info(&self) -> &CommandInfo;
    fn execute(&self, ctx: &CommandContext, args: Option<&str>) -> CommandResult;
    fn completion(&self, partial: &str) -> Vec<CompletionCandidate>;  // 自动补全
}

struct CommandInfo {
    name: String,
    aliases: Vec<String>,
    usage: String,             // 如 "/agent [name|new]"
    description: String,
    group: CommandGroup,       // Core | Session | Config | Agent
    takes_args: bool,
    sub_commands: Vec<SlashCommand>,  // 子命令（如 /agent new, /agent switch）
}

// 命令解析器（参考 gemini-cli 的 SlashCommandResolver）
struct CommandResolver {
    commands: HashMap<String, Arc<dyn SlashCommand>>,
    // 基于优先级的名称冲突解决
    // 内置命令保留原名，外部命令加前缀
}

impl CommandResolver {
    // 最优匹配（模糊匹配 + 前缀匹配 + 别名匹配）
    fn resolve(&self, input: &str) -> Option<ResolvedCommand>;
    // 自动补全候选项（按匹配分数排序）
    fn complete(&self, partial: &str) -> Vec<CompletionCandidate>;
}
```

**自动补全系统细化（综合各项目）：**

```
用户输入 "/a" + Tab →
  匹配: /agent (别名匹配: a→agent)
  补全候选（按优先级排序）:
    1. 精确前缀匹配: /agent
    2. 别名匹配: /agent (alias: a)
    3. 模糊匹配: 无其他匹配
  补全结果: /agent  (同时显示子命令: new, switch, list)

用户输入 "/agent " + Tab →
  子命令补全:
    - new     — 新建自定义 agent
    - switch  — 切换到已有 agent
    - list    — 列出所有 agent
```

## 8. 自定义 Agent Profile

每个 profile 至少包含：

- **agent 名称**（用户自定义输入）
- **每个工具的权限**（`ask` / `auto-accept` / `disabled`，逐工具独立设置）
- **`{agent名称}_agent.md`** 位于全局配置文件目录作为该自定义 agent 的独立约束文件（用户可自行编辑写入自定义规则和系统提示）

**内置 3 个 agent 的约束文件：**
- `plan_agent.md` — Plan agent 的约束规则
- `build_agent.md` — Build agent 的约束规则
- `yolo_agent.md` — Yolo agent 的约束规则

**参考实现要点：**

- **OpenCode 的 Agent 系统**：`Agent.Info = { id, model?, system?, description?, mode: "subagent" | "primary" | "all", hidden, color?, steps?, permissions: Ruleset }`。Agent 从多层加载：`.opencode/opencode.jsonc` 的 `agents` 键 + `.opencode/agent/*.md` 文件（YAML frontmatter 定义属性，body 作为系统提示）+ 内置默认值。`Agent.select(id?)` 过滤 subagent/hidden，返回可用 agent 列表。

- **CodeWhale 的 WorkerRuntimeProfile**：`{ role, permissions: PermissionSet, shell: ShellPolicy, tools: ToolScope, model: ModelRoute, denied_tools, max_spawn_depth, max_steps }`。子 agent 派生时能力取交集（AND）——子 agent 永远无法获得父 agent 没有的能力。`for_role()` 返回预设角色 profile（Explore/Plan/Review/Verifier/Implementer/General/Custom）。

- **MiMo-Code 的 Agent 定义**：agent 的 `permission` 字段（Ruleset）+ `hardPermission`（不可绕过的安全底线），`runtimePermission()` 按 `agent + session + hardPermission` 顺序合并。

- **claude-code 的 Mode/Persona 系统**：`CCBMode = { name, slug, description, icon, systemPrompt, model, ui, permissions, responseStyle }`。从 `~/.claude/modes/` 加载自定义 mode（YAML 或 Markdown frontmatter）。

**Joker Agent Profile 设计细化：**

```rust
struct AgentProfile {
    // 基本信息
    name: String,                    // agent 名称（唯一标识）
    display_name: String,            // TUI 展示名
    description: String,             // 简要描述

    // 模型配置
    model: Option<ModelRef>,         // 可选模型覆盖（不填则用全局默认）

    // 工具权限（核心特色）
    tool_permissions: HashMap<ToolName, PermissionSetting>,
    // PermissionSetting = Ask | AutoAccept | Disabled

    // 约束文件路径
    constraint_file: PathBuf,        // "{name}_agent.md"，用户可自由编辑
    // 约束文件包含：
    //   - 系统提示（追加到全局系统提示之后）
    //   - 自定义规则（如 "always use git commit after file changes"）
    //   - 行为约束（如 "never modify files outside src/"）

    // 执行限制
    max_steps: Option<usize>,        // 最大工具调用步数（超出强制停止）
    max_spawn_depth: Option<usize>,  // 子 agent 最大嵌套深度

    // 可见性
    hidden: bool,                    // 是否在 /agent 列表中隐藏
    mode: AgentMode,                 // Primary | Subagent | All
}

// 子 agent 派生（参考 CodeWhale 的能力交集原则）
impl AgentProfile {
    fn derive_child(&self, child_profile: &AgentProfile) -> AgentProfile {
        // 权限取最严格（AND）
        // tool_permissions：子不能超过父（disabled 优先于 ask 优先于 auto-accept）
        // max_spawn_depth：父的值 - 1
        // max_steps：取 min(父, 子)
        // 原则：子 agent 永远无法获得父 agent 没有的能力
    }
}
```

**Agent 配置文件存储结构：**

```
~/.joker/
├── config.toml              # 全局配置
├── auth.json                # API key 存储（0o600 权限，参考 pi）
├── agents/                  # Agent 约束文件目录
│   ├── plan_agent.md        # 内置 Plan agent
│   ├── build_agent.md       # 内置 Build agent
│   ├── yolo_agent.md        # 内置 Yolo agent
│   └── {custom}_agent.md    # 用户自定义 agent
├── sessions/                # 会话存储
│   └── <cwd_hash>/
│       └── <session_id>.jsonl
└── skills/                  # 用户自定义 skills
```

## 9. 当前仓库实态调研

本章基于当前 workspace 源码整理，目标是把 Joker 从“设计蓝图”落到“代码已经做到什么、边界在哪里、下一步应该怎么接”。当前仓库不是单一 TUI demo，而是一个拆分较清晰的 Rust workspace：

```text
crates/
├── joker            # 核心 agent kernel：协议、模型 trait、工具 trait、权限、上下文、会话、事件
├── joker-provider   # provider 实现：OpenAI-compatible、Anthropic、Google，以及 Route/Profile/Discovery
├── joker-config     # joker.toml 文件 schema、CLI override 合并、RuntimeConfig
├── joker-tools      # 内置工具实现：文件、搜索、shell、web、memory、todo
├── joker-mcp        # MCP JSON-RPC client、stdio transport、MCP tool adapter
└── joker-tui        # Ratatui/Crossterm TUI、slash commands、agent driver、UI state
```

### 9.1 Workspace 边界

当前依赖方向基本符合“核心内核不依赖外设”的目标：

- `joker` 定义稳定核心抽象：`Model`、`Tool`、`ToolRegistry`、`ToolPolicy`、`PermissionEngine`、`ContextBuilder`、`SessionStore`、`Observer`、`Event`、`Conversation` 等。
- `joker-provider` 依赖 `joker::Model`，把真实 HTTP/SSE provider 包装成核心可消费的流。
- `joker-tools` 依赖 `joker::Tool`，把文件系统、shell、web、memory、todo 包装为工具。
- `joker-mcp` 依赖 `joker::Tool`，把外部 MCP 工具动态适配进同一个工具注册表。
- `joker-config` 依赖 provider profile / route 类型，负责把文件配置和 CLI 参数解析成 runtime 选择。
- `joker-tui` 是应用层组合器：构造 model、tool registry、permission policy、context builder、session store，并把 core events 投影到 UI。

这条边界很重要：任何新功能默认应先判断它属于 core contract、provider adapter、tool implementation、config schema 还是 TUI orchestration。不要把 provider 特例塞进 `joker`，也不要让工具自己处理交互审批。

### 9.2 核心 crate：`crates/joker`

`joker` crate 是当前最稳定的 API 层。`lib.rs` 暴露的主要面向用户接口包括：

- Agent loop：`Agent`、`AgentBuilder`、`AgentRuntime`、`Op`
- Agent 配置：`AgentConfig`、`ExecutionMode`、`RetryConfig`、`RunLimits`
- 对话协议：`Conversation`、`Message`、`Content`、`ToolCall`、`ToolResult`、`Usage`、`StopReason`
- 模型接口：`Model`、`ModelRequest`、`ModelResponseEvent`、`ModelStream`
- 工具接口：`Tool`、`ToolDefinition`、`ToolAnnotations`、`ToolCapability`、`ToolRegistry`
- 权限接口：`ToolPolicy`、`PermissionPolicy`、`PermissionEngine`、`AgentPermission`、`PermissionSetting`
- 上下文接口：`ContextBuilder` 系列、`assemble_system_prompt`
- 会话接口：`SessionStore`、`JsonlSessionStore`
- 事件接口：`Observer`、`Event`、`RecordingObserver`
- 扩展接口：`Hook`、`HookRegistry`、`Skill`、`SkillRegistry`

#### Agent 内层工具循环

`Agent::run` 是核心 inner loop：

```text
RunRequest
  -> 如果 conversation 为空，把 input 放入 user message
  -> loop:
       1. 检查 cancellation token
       2. 检查 max_steps
       3. run_turn()
       4. 如果没有 tool_calls，返回 RunOutcome
       5. 检查 max_tool_calls
       6. execute_tool_calls()
       7. 把 ToolResult 作为 Tool message 压回 conversation
       8. 继续下一轮模型调用
```

`run_turn()` 的真实顺序是：

```text
TurnStarted event
-> context_builder.build()
-> ModelStarted event
-> model.stream(ModelRequest { messages, tools })
-> collect_model_output()
   -> TextDelta / ReasoningDelta / ToolCall / Finished
-> ModelFinished / Usage / TurnDone events
-> assistant message 入 conversation
-> 返回 pending tool calls
```

当前 retry 分两层：

- `Agent::run_turn` 对 stream 初始化错误最多重试 `RetryConfig::max_stream_retries`，默认 4 次，指数退避。
- 对空输出最多重试 `max_zero_output_retries`，默认 3 次。
- `joker-provider::ReconnectingModel` 另提供 provider stream 包装：只有在还没有任何输出事件时才重连，避免重复输出。

#### Runtime 外层 Op loop

`AgentRuntime::run` 在 `Agent` 外包一层 `Op` 队列和 steer/follow-up 队列，已实现的 `Op` 包括：

- `SendMessage { text }`
- `Cancel`
- `Interrupt { text }`
- `Approve { approved, remember_for_session, reason }`
- `Compact`
- `SwitchAgent { name }`
- `Shutdown`

它的设计已经接近第 2 章提出的 SQ/EQ 模式，但当前 TUI 主要直接使用 `Agent::run`，没有全面走 `AgentRuntime::run`。因此：

- core 已经有 Op loop 能力。
- TUI 当前仍是较轻的 spawn-run 模式。
- 后续如果要支持运行中 steer、真正的 compact op、agent/model mid-run switch，应优先把 TUI driver 改成 `AgentRuntime` 驱动，而不是复制一套 loop。

#### 并行工具执行

`AgentConfig.execution_mode` 支持：

- `Sequential`
- `ParallelWhenSafe`

当 execution mode 是 `ParallelWhenSafe`，且一批 tool calls 的所有工具定义都是 `ToolExecution::ParallelSafe` 时，`execute_tool_calls` 会用 `join_all` 并发执行。否则按顺序执行。当前内置 read/search 类工具标记为 parallel-safe，写文件、编辑、shell、patch、todo、network 多数仍是 sequential。

### 9.3 对话协议与 Provider 无关性

核心协议模型在 `protocol.rs`：

```rust
Conversation -> Vec<Message>
Message { role, content: Vec<Content> }
Content = Text | Reasoning | ToolCall | ToolResult
ToolCall { id, name, arguments }
ToolResult { call_id, name, output, is_error }
StopReason = Stop | ToolUse | Length | Cancelled | LimitReached
```

这个结构是 provider-neutral 的，provider crate 负责做 wire-format transform：

- OpenAI-compatible：system/user/assistant/tool 消息转 chat completions。
- Anthropic：system 放 top-level `system`，tool result 转 `tool_result` block。
- Google Gemini：message content 转 `contents.parts`，tool call 转 `functionCall`，tool result 转 `functionResponse`。

`joker-provider::transform` 已经实现一组跨 provider 消息修复：

- 合并 system messages。
- 过滤空消息。
- 针对 Anthropic/Bedrock/Mistral scrub tool call id。
- 对 DeepSeek/QwQ/R1 类模型补空 reasoning block。
- 合并连续 text parts。

这说明“Provider 适配层不能污染内核”的方向已经落地：核心不认识 Anthropic/Gemini/OpenAI 的 wire shape。

### 9.4 Provider 层现状：`crates/joker-provider`

Provider 层当前不再是 DeepSeek-only。已实现：

- `OpenAiCompatibleModel`
- `AnthropicModel`
- `GoogleModel`
- `Route`
- `ProviderProfile`
- `discover_models`
- `detect_vendor`
- `guess_protocol`
- `guess_framing`
- `ReconnectingModel`

#### Route 抽象

`Route` 当前字段：

```rust
Route {
    id,
    protocol: Protocol,
    base_url,
    auth: Auth,
    framing: Framing,
    default_model,
}
```

`Protocol` 包括：

- `ChatCompletions`
- `AnthropicMessages`
- `GoogleGemini`

`AuthScheme` 包括：

- `Bearer`
- `ApiKey { header }`
- `None`

`CredentialSource` 包括：

- `None`
- `EnvVar(String)`
- `Value(String)`

这已经是 OpenCode 四轴设计的简化版：Protocol + Endpoint + Auth + Framing 组合成可 materialize 的 model route。

#### 内置 provider profiles

当前内置 profile：

| Provider | Protocol | Base URL | Env |
|---|---|---|---|
| DeepSeek | ChatCompletions | `https://api.deepseek.com` | `DEEPSEEK_API_KEY` |
| Alibaba DashScope | ChatCompletions | `https://dashscope.aliyuncs.com/compatible-mode/v1` | `ALIBABA_API_KEY` |
| ZhipuAI | ChatCompletions | `https://open.bigmodel.cn/api/paas/v4` | `ZHIPUAI_API_KEY` |
| Moonshot/Kimi | ChatCompletions | `https://api.moonshot.cn/v1` | `MOONSHOT_API_KEY` |
| Baidu/Qianfan | ChatCompletions | `https://qianfan.baidubce.com/v2` | `BAIDU_API_KEY` |
| Anthropic | AnthropicMessages | `https://api.anthropic.com` | `ANTHROPIC_API_KEY` |
| Google | GoogleGemini | `https://generativelanguage.googleapis.com` | `GOOGLE_GENERATIVE_AI_API_KEY` |

TUI `/provider` 命令也展示这些 profile，并额外支持 `scripted`。

#### OpenAI-compatible 实现

`OpenAiCompatibleModel` 当前能力：

- 拼接 `{base_url}/chat/completions`
- 发送 streaming chat completions request
- 序列化 Joker messages 和 tools
- 解析 SSE `data:` 行
- 支持 `content` delta
- 支持 `reasoning_content` delta
- 累积 streaming tool call function arguments，直到 JSON 完整后发出 `ToolCall`
- 根据 `finish_reason` 映射 `StopReason`
- 对 Alibaba / ZhipuAI reasoning 模型合并 provider-specific `extra_body`
- 支持 `/models` 检测，失败时回退到配置模型

当前缺口：

- Usage 在 OpenAI-compatible streaming parser 中基本还是默认值，未完整解析 provider usage chunk。
- Tool call streaming 当前一次只发出第一个完成的 tool call，对复杂多工具并发 chunk 还需要更严格测试。
- `apply_patch` 类工具的输出格式还没有 provider-specific token budget 优化。

#### Anthropic / Google 实现

Anthropic provider 已支持：

- `x-api-key` + `anthropic-version`
- `/v1/messages`
- streaming SSE
- text / thinking / tool use / usage 映射
- tools schema 转 Anthropic format

Google provider 已支持：

- `x-goog-api-key`
- `models/{model}:streamGenerateContent?alt=sse`
- `systemInstruction`
- `functionCall` / `functionResponse`
- `thought: true` reasoning block

后续要补的不是“是否支持”，而是互操作细节：

- tool schema canonicalization
- 多模态内容
- provider-specific usage/caching
- 错误分类：auth、rate limit、quota、model not found、context length

### 9.5 Config 层现状：`crates/joker-config`

`joker-config` 当前解析项目本地 `joker.toml`，CLI overrides 优先：

```text
CLI flags
> joker.toml
> built-in defaults
```

`FileConfig` schema 包括：

- `provider`
- `model`
- `base_url`
- `api_key_env`
- `scripted_response`
- `demo_tool`
- `[providers.<name>]`
- `[agent.<name>]`
- `[mcp_servers.<name>]`

`RuntimeConfig` 当前只保留运行期真正需要的精简字段：

```rust
RuntimeConfig {
    provider: ProviderSelection,
    scripted_response: String,
    demo_tool: bool,
}
```

这里有一个重要现状：`FileConfig` 支持 agent 和 MCP 配置，但 `RuntimeConfig::to_file_config()` 会把 agent 和 mcp servers 转成空 map。因此 TUI `AgentDriver::new_with_agents_dir()` 里通过 `runtime_config.to_file_config().agent` 注册 custom agent 的逻辑，实际拿不到原始 file config 中的 agent。`/agent new` 会直接读写 `joker.toml`，但运行期 reload/driver sync 仍不完整。

下一步应该调整：

- `RuntimeConfig` 保存 `agents` 和 `mcp_servers` 的 resolved 结果，或者 `AgentDriver` 直接持有 `FileConfig` / `ConfigStore`。
- `ConfigStore::load` 返回结构应能同时保留“resolved runtime”和“原始可持久化配置”。
- `/config save` 相关旧文档和当前命令实现不一致：当前 command registry 没有 `/config` 命令，只有 provider/model 的 session-level change 和部分直接写配置逻辑。

### 9.6 Credential 存储现状

`joker::CredentialStore` 已实现：

- 内存 map：`provider_id -> api_key`
- 可选文件后端：`CredentialStore::with_file(path)`
- `get(provider_id)` 先查 store，再回退 `PROVIDER_ID_API_KEY`
- `save()` 写 JSON，并在 Unix 设置 `0o600`

TUI 启动时：

- 使用 `JOKER_HOME` 或 `$HOME/.joker`
- 设置 credential path 为 `~/.joker/auth.json`
- provider 切换后如果 store 没有 key 且 env var 不存在，会进入 API key input overlay
- 用户输入后保存到 credential store，并把 route auth 改为 `CredentialSource::Value`
- 随后触发 model discovery

需要注意：

- config 文档中“Secrets are not stored in joker.toml”仍成立。
- 当前 credential store 没有文件锁，也没有 OAuth refresh。
- `CredentialStore::get` 的 env fallback 使用 `PROVIDER_ID_API_KEY`，而 `Route` 本身可能带 profile-specific env var；TUI provider sync 走的是 route credentials，因此两者并存。后续要统一 credential resolution chain，避免同一个 provider 有两套 env var 逻辑。

### 9.7 工具系统现状：`crates/joker-tools`

内置工具 registry 分三层：

- `readonly_tool_registry`：`list_files`、`read_file`、`grep`、`glob`
- `writeable_tool_registry`：readonly + `write_file`、`edit_file`、`shell`、`apply_patch`、`fetch_url`、`todo_write`
- `all_tool_registry`：writeable + `web_search` + `memory_read` + `memory_write`

所有工具统一实现 `joker::Tool`：

```rust
fn definition(&self) -> ToolDefinition;
fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_>;
```

工具只声明 metadata 和执行逻辑，不直接做用户审批。审批由 `ToolPolicy` / `PermissionEngine` 决定。

#### Workspace sandbox

`WorkspaceTool` 是所有文件工具的安全边界：

- `resolve_read(path)`：目标必须存在，canonicalize 后必须在 workspace root 内。
- `resolve_write(path)`：允许目标不存在，但会向上寻找已存在祖先并 canonicalize，确保写入路径仍在 workspace 内。
- `parse_args` 统一把 JSON args 转 struct。
- `truncate_at_char_boundary` 避免 UTF-8 截断。
- `detect_line_ending` 辅助保留 LF/CRLF。

新增文件工具必须复用 `WorkspaceTool`，不能自己手写 path join。

#### 文件读取

`read_file` 当前能力：

- UTF-8 文本读取
- `offset` / `limit` 行窗口
- `max_bytes`
- `code_fence`
- 10MB 文件上限
- 二进制检测
- 图片扩展名返回 base64 data URL
- `allow_binary` 时二进制返回 base64
- 输出 `line_ending` 和 `bom` 信息

当前缺口：

- 对非 UTF-8 文本没有编码探测。
- BOM 保留通过字符串切片处理，后续要确认 UTF-8 BOM 边界逻辑持续正确。
- 大文件建议优先 grep/glob，但没有自动提出下一步建议结构。

#### 文件写入

`write_file` 当前能力：

- 创建父目录
- 覆盖写 UTF-8 文本
- 标记 mutating + `ApprovalRequirement::Required`

当前缺口：

- 没有写前读取验证。
- 没有 stale-file 检查。
- 没有换行符/BOM 继承。
- 没有 diff preview。

#### 编辑工具

`edit_file` 是当前实现最复杂的文件工具。它支持：

- `old_string` / `new_string` / `replace_all`
- exact match
- line-trimmed match
- block-anchor match
- whitespace-normalized match
- indentation-flexible match
- escape-normalized match
- trimmed-boundary match
- context-aware match
- multi-occurrence match
- disproportionate span 拒绝
- 保留原文件 LF/CRLF
- 写前 stale check：如果 read 和 write 之间文件变化，则拒绝

这很适合 LLM 实际编辑，但也意味着行为不再是纯“精确字符串替换”。后续文档和测试要明确：

- 默认应鼓励模型提供精确 `old_string`。
- fuzzy 策略是兜底，不应掩盖错误上下文。
- 对多处匹配，如果没有 `replace_all`，应要求更大上下文。

#### Patch 工具

`apply_patch` 当前是单文件 unified diff patch：

- 输入 `path` + `patch`
- 读取一个目标文件
- 解析 `@@ -start,count +start,count @@` 风格 hunk
- 按 context/removal/addition 应用
- 失败时返回 tool error

当前缺口：

- 不是完整 Codex `*** Begin Patch` grammar。
- 不支持一个 patch 同时 add/update/delete 多文件。
- hunk 校验和失败报告较轻。
- 没有 diff summary。

如果后续要实现与 Codex 风格一致的 `apply_patch`，建议新增 parser 模块和 golden tests，不要在当前函数里继续堆字符串逻辑。

#### Shell 工具

`shell` 当前能力：

- 在 workspace root 下执行
- 支持 `timeout_secs`，默认 120 秒
- 支持 `bg` 后台执行并立即返回 PID
- Unix 下设置 process group
- timeout 后尝试 kill process group
- stdout/stderr 收集
- 输出截断：stdout 64KB、stderr 8KB
- stdout 超限时 spill 到 `/tmp/joker-shell-spill`
- 检测 blocked env vars：`LD_PRELOAD`、`LD_LIBRARY_PATH`、`LD_AUDIT`、`LD_DEBUG`、`SHELL`、`HOME`、`PATH`
- 检测 chaining / command substitution / path traversal / background execution warning

当前缺口：

- shell safety warnings 只是输出字段，不会直接阻断；真正阻断由 TUI `ChainPolicy` 的 deny rules 和审批策略决定。
- 没有 PTY。
- 后台 job 没有持续状态查询工具。
- 没有 sandbox profile 或 escalation retry。

#### 搜索工具

`grep`：

- 优先调用 `rg --json`
- 支持 `path`、`max_matches`、`context_lines`、`include`、`exclude`
- ripgrep 不可用或无结果时 fallback 到纯 Rust 遍历
- fallback 跳过 >1MB 文件

`glob`：

- 使用 `ignore::WalkBuilder`
- `.gitignore` / global gitignore 感知
- hidden false、follow links false
- 支持 `max_depth`、`max_results`

`list_files`：

- 支持 recursive / non-recursive
- 当前 recursive 模式关闭 git ignore (`git_ignore(false)`)，这一点与 glob 不同。后续如果希望默认尊重 `.gitignore`，要改实现并更新测试。

#### Web 工具

`web_search`：

- DuckDuckGo HTML 后端，无 API key
- 返回 title/url/snippet
- 429 映射为 `RateLimited`

`fetch_url`：

- 只允许 http/https
- 私有/保留 IP SSRF 保护
- 1MB fetch 上限
- HTML 粗提取文本
- 输出最多 64KB 文本

当前缺口：

- HTML 提取是轻量字符串扫描，不是成熟 Readability/Markdown 提取。
- DNS rebinding 保护不完整。
- 没有 robots/cache/source citation policy。

#### Memory / Todo

`memory_read` / `memory_write`：

- 存储在 workspace `.joker-memory/MEMORY.md`
- 支持 YAML-like frontmatter 解析
- 支持 `@path` 和 `@path#L10-L20` 引用展开
- 支持以 `#` 开头的 quick append 语义

`todo_write`：

- 存储在 workspace `.joker/todos.json`
- 按 `id` merge update
- 默认保留未提及 todo
- 状态门控：`pending -> in_progress/completed`，`in_progress -> completed`，不允许回退；`overwrite=true` 可绕过

注意：`todo_write` metadata 的 `default_approval` 是 Auto，但它是 mutating。是否应 auto 取决于 agent profile。build agent 仍会 ask，yolo 会 auto，plan disabled。

### 9.8 权限系统现状

当前权限系统有两套相互补充的层：

1. `policy.rs`：通用 `ToolPolicy` trait、rule-based `PermissionPolicy`、approval channel。
2. `permission_engine.rs`：agent profile 权限、hard permission、session grant、工具 materialization。

#### `ToolPolicy` 层

`ToolPolicy::evaluate(ToolPolicyRequest) -> ToolDecision`：

- `Allow`
- `Deny { reason }`
- `Ask { request_id, reason }`

`PermissionPolicy` 支持：

- explicit rules，last match wins
- session allows
- persisted allows
- shell chain / redirect 自动 Ask
- mutating tool 默认 Ask
- read-only 默认 Allow

TUI 还额外构造了 `ChainPolicy`：

- 第一层 safety policy 硬拒绝 `rm -rf`、`sudo`
- 第二层 engine policy 执行 agent 权限
- 另有 shell redirect/pipeline patterns 导致 Allow 降级 Ask

#### `PermissionEngine` 层

`PermissionEngine::evaluate` 顺序：

1. path-level hard permission rules
2. blanket hard permission
3. agent-level Disabled
4. agent-level AutoAccept
5. session grant
6. agent-level Ask
7. tool annotation default：mutating Ask，read-only Allow

`materialize_tools(agent, all_tools)` 会把 disabled 或 hard-disabled 的工具从模型可见工具列表中过滤掉。这是安全性和模型行为质量都很关键的点：禁止工具不应该只是调用时报错，最好根本不暴露给模型。

当前需要注意的实现细节：

- `PermissionEngine` 的 persisted session grants 可选保存到 `grants.json`，但 TUI 默认创建的是 `PermissionEngine::new()`，没有启用持久化。
- `SharedApprovalChannel` 也有自己的 `session_allows`，`EnginePolicy` 会检查它。也就是说 session grant 目前有 engine 内部和 approval channel 两条路径，后续需要收敛。
- approval channel 当前一次只支持一个 pending request。

#### Built-in agents

`builtin_agent_profiles` 当前定义：

| Agent | 核心语义 |
|---|---|
| `plan` | read/search/memory auto，web ask，写入/shell/patch/todo disabled，并有 hard_permission 禁止 mutating |
| `build` | 所有内置工具 ask |
| `yolo` | 文件读写、shell、todo、memory auto，web ask |

`plan` 有一个 hard rule 允许 `plans/*.md`，但因为当前工具权限中 `write_file/edit_file/apply_patch/todo_write/shell` 已被 Disabled，且 materialization 会先跳过 disabled 工具，实际“plan 可写 plans/*.md”这条例外未必能生效。后续如果要让 plan 写计划文件，需要调整：

- 不把对应写工具在 agent-level 设置为 Disabled，而是通过 hard rules 控制路径；或
- materialize 时保留有 allow override 的工具；或
- 单独提供 `plan_write` 工具。

### 9.9 TUI 层现状：`crates/joker-tui`

TUI 是当前最终用户入口。主要模块：

- `cli.rs`：clap CLI flags
- `terminal.rs`：raw mode、alternate screen、event loop、dialog confirm、run spawn
- `app.rs`：UI state machine
- `driver.rs`：AgentDriver，负责构造 Agent
- `commands/*`：slash command 系统
- `widgets/*`：布局、composer、transcript、selector、command palette

#### CLI flags

当前 binary 支持：

- `--prompt`
- `--no-alt-screen`
- `--config`
- `--provider`
- `--model`
- `--base-url`
- `--api-key-env`
- `--scripted-response`
- `--demo-tool`

`--prompt` 会在启动后自动提交首条消息。

#### TUI runtime setup

`run_tui()` 启动流程：

```text
setup terminal
-> spawn terminal event thread
-> spawn tick event task
-> App::with_config()
-> workspace = current_dir()
-> agents_dir = joker_home_dir()/agents
-> AgentDriver::new_with_agents_dir()
-> driver.init_mcp_servers()
-> JsonlSessionStore at .joker/sessions
-> credential store at ~/.joker/auth.json
-> optional initial prompt
-> event loop: key/tick/agent/model-discovery/run-completed
```

`joker_home_dir()` 优先级：

```text
JOKER_HOME
> $HOME/.joker
> current_dir/.joker
```

注意：session store 在项目 `.joker/sessions`，agent constraint / auth 在 home `.joker`。这是当前真实布局。

#### AgentDriver 装配链

每次 run 前，`AgentDriver::build_agent()`：

```text
build_model()
-> ChannelObserver
-> Agent::new(model)
-> all_tool_registry(workspace)
-> 加入 MCP tools
-> permission_engine.materialize_tools(active_agent)
-> ChainPolicy(safety_policy, engine_policy)
-> assemble_system_prompt(active_agent, None, None)
-> PrefixedContextBuilder(system_prompt, Passthrough or Compacting)
```

这说明“API first / composable kernel”已经在应用层落地。后续要加 HTTP server、one-shot CLI 或 ACP/MCP server，应复用这条装配链，不要重写 agent loop。

#### Slash commands

当前注册命令：

- `/exit`
- `/provider [name]`
- `/model [name]`
- `/sessions [list|load <id>|delete <id>]`
- `/compact`
- `/agent [new|switch|list]`

`CommandRegistry` 支持：

- exact match dispatch
- prefix suggestion
- fuzzy suggestion（`fuzzy-matcher`）
- command-specific argument completion
- 空 slash 输入生成帮助文本

当前没有独立 `/help`、`/config`、`/cancel` 命令。取消运行通过键盘 `Ctrl-C` / `Esc` 触发 `AppAction::Cancel`。

#### Agent 新建向导

`/agent new` 当前行为：

- 进入 dialog wizard
- 收集 agent name
- 收集 tool permissions
- 在 driver.permission_engine 注册新 `AgentPermission`
- 写入 `joker.toml` 的 `[agent.<name>]`
- 写入 `~/.joker/agents/{name}_agent.md`
- 更新 app.agent_names

当前缺口：

- wizard 创建的 config 持久化了，但 `RuntimeConfig` 没有保留 agent config，重启后 custom agent 加载链不完整。
- 没有读取 `{name}_agent.md` 的用户编辑内容。`assemble_system_prompt()` 当前只返回内置 agent prompt；custom agent constraint file 尚未接入。
- 内置 agent 文件写到 `~/.joker/agents`，但 `builtin_constraint_file_content()` 只是内置静态字符串；外部文件修改不会影响 prompt。

这应作为 agent 系统下一阶段优先修复。

### 9.10 Session / Persistence 现状

`JsonlSessionStore` 是 append-ish JSONL 文件实现，但当前 `save()` 实际会重写 `{id}.jsonl` 完整内容，并更新 `index.json`。

每个 session 文件：

```text
line 1: header JSON
line 2..n: serialized Message JSON
```

`SessionData` 字段：

- `id`
- `label`
- `created_at`
- `updated_at`
- `model`
- `agent_name`
- `parent_id`
- `root_id`
- `conversation`

`SessionStore` trait 支持：

- `save`
- `load`
- `list`
- `delete`
- `fork`
- `path_to_root`
- `children`

TUI `/sessions` 当前支持 list/load/delete，不支持 fork/path navigation UI。

当前缺口：

- `App::save_current_session` 已在 `RunCompleted` 事件中调用并保存到 session store，但每次都是新建 session id，而非更新原 loaded session 或正确设置 fork/parent 关系。如果用户通过 `/sessions load` 继续对话，下一轮会写入全新记录，原链丢失。
- JSONL 不是事件溯源，只是 conversation snapshot；SessionData 本身也不支持增量追加。
- `delete` 会删除直接 children 文件，但不是递归删除整棵树。

### 9.11 Context / Compaction 现状

上下文构建器包括：

- `PassthroughContextBuilder`
- `FixedWindowContextBuilder`
- `SummaryContextBuilder`
- `CompactingContextBuilder`
- `PrefixedContextBuilder`

`ContextLimits` 默认：

- `max_messages = 64`
- `max_text_bytes = 64KB`
- `max_tool_result_bytes = 64KB`

`CompactingContextBuilder`：

- 总是先跑 `micro_dedup_messages`
- 用 `estimate_tokens` 粗略估算 tokens（约 4 chars/token）
- `ContextThresholds` 默认：48k / 76.8k / 86.4k tokens
- `Soft`：返回完整上下文
- `Compact`：生成 heuristic summary system message + recent messages
- `Force`：保留 system messages + 最近少量消息

当前 summary 是 heuristic，不是 LLM summary：

- 统计 user/assistant/tool 数量
- 提取首条 user message
- 加一句 earlier messages summarized

`/compact` 目前只是设置 flag，下一次 run 使用 `CompactingContextBuilder`。`AgentRuntime::Op::Compact` 当前只发 CompactionStarted/Done dummy events，未真正压缩 active conversation。

下一步优先级：

1. 明确 context builder 是否允许超出 hard limits 后自动降级，而不是直接 `LimitExceeded`。
2. 接入真正 LLM summary 或 pluggable summarizer。
3. TUI 中让 `/compact` 对当前 loaded conversation 立即生效，而不是下一次 run 才生效。
4. `micro_dedup_messages` 的路径识别目前依赖输出字符串形态，应改成结构化识别 tool result JSON。

### 9.12 MCP 现状：`crates/joker-mcp`

当前 MCP 支持 stdio server：

- `StdioTransport::spawn(command, args)`
- newline-delimited JSON-RPC
- `initialize`
- `notifications/initialized`
- `tools/list`
- `tools/call`
- `close`

`McpToolAdapter`：

- 把 MCP tool name 映射为 Joker tool name：`mcp_{tool_name}`
- MCP input schema 原样作为 Joker `input_schema`
- 调用时把 Joker args 透传给 `tools/call`
- text content 合并为字符串 result
- `isError` 映射为 `{ "error": ... }`
- metadata 标记为 Network + Auto

当前缺口：

- 命名不是第 2/5 章规划的 `mcp__server__tool`，会有多 server 同名冲突。
- 只实现 stdio，没有 SSE / Streamable HTTP。
- 没有工具列表变化通知处理。
- 没有断线重连。
- MCP adapter 默认 Auto + Network，但真实权限应由 agent profile 决定。当前内置 profile 没列出动态 MCP 工具名，因此会落到默认：read-only false? adapter mutating false，所以默认 Allow。这可能过宽，应把 MCP 工具默认 `ApprovalRequirement::Suggest` 或在 materialize/policy 层按 `mcp_` 前缀统一 Ask。

### 9.13 Hooks / Skills 现状

`Hook` trait 已定义：

- `before_tool_call`
- `after_tool_call`
- `before_provider_request`
- `on_session_start`
- `on_session_end`

`HookRegistry` 能顺序执行 hooks。

`Skill` / `SkillRegistry` 已定义：

- name
- description
- path patterns
- prompt_content
- allowed_tools
- path matching
- system_prompt generation

当前缺口：

- Agent loop 尚未调用 `HookRegistry`。
- TUI/driver 尚未加载 skills。
- `allowed_tools` 尚未和 tool materialization 结合。
- 没有 skills 文件格式、目录加载、显式调用或隐式触发机制。

因此 hooks/skills 目前是 API seed，不是完整产品能力。

### 9.14 测试覆盖现状

当前测试文件覆盖面较广：

`crates/joker/tests/`：

- cancellation
- limits
- event_order
- text_turn
- credential_store
- op_loop
- context_builder
- tool_execution
- agent_profiles
- hooks
- provider_events
- tool_turn

`crates/joker-tui/tests/`：

- driver
- render_smoke
- app_state
- e2e

模块内单元测试还覆盖：

- permission engine hard permission / session grants
- session save/load/list/delete/fork/path
- provider route/auth/model discovery helper
- transform message normalization
- skill glob matching
- todo merge/gating
- config resolve/store

建议后续新增的关键测试：

- `joker-tools` integration tests：workspace escape、write stale behavior、edit fuzzy false positive、shell timeout kill、fetch_url SSRF。
- provider golden tests：OpenAI/Anthropic/Google request body snapshots、SSE streaming parser 多 tool call chunks。
- TUI command tests：`/agent new` 持久化后重载、API key overlay、model discovery result application。
- MCP tests：mock stdio server，覆盖 initialize/list/call、同名工具冲突。
- Session lifecycle tests：一次 TUI run 完成后是否保存 session。

### 9.15 参考项目调研对比

本章基于 `../agents/` 中 8 个参考项目的分析文件，整理出与 Joker 当前实现直接相关的跨项目模式对比。以下结论已反映到第 10 章中各 P0/P1/P2 项目的细化中。

#### 各项目最值得借鉴的模块

| 项目 | 最值得借鉴的模式 | 对应 Joker 缺口 |
|---|---|---|
| **pi** | 会话树模型、LLM 压缩、工具调用生命周期的钩子分阶段（prepare→execute→finalize） | 9.10 Session / 10.4 Session；9.11 Compaction / 10.9 事件溯源 |
| **CodeWhale** | `ToolCapability` 枚举取代单一 bool、`ToolCallRuntime` 读写锁控制并发（读=并行，写=串行）、ExecPolicy 三层规则集、MCP qualified name `mcp__server__tool` | 9.12 MCP / 10.11 MCP；9.8 权限 / 10.6 权限收敛 |
| **OpenCode** | 事件溯源 EventV2 + SessionProjector 投影系统、ACP 协议层、Directory Snapshot 多源头配置加载、Steer/Queue 输入分拣 | 10.9 事件溯源；长期补充：创建新入口 crate `joker-server` |
| **claude-code** | MCP server 断开重连 + `toolsChanged` 事件通知、`mcp_auth` 认证工具、Workflow Engine 确定性脚本编排 | 9.12 MCP / 10.11 MCP；10.10 Skills/Hooks |
| **codex** | AppServer 内进程 go-between 模式、Linux bwrap + macOS Seatbelt 多层沙箱、Plugin/Marketplace 系统 | 中远期 sandbox / plugin |
| **DeepSeek-Reasonix** | Controller 模式（传输无关的会话驱动层，独立锁按子系统拆分）、Subagent 嵌套深度控制、Checkpoint/Time Machine 时光机 | 10.5 TUI 驱动切 AgentRuntime；中远期 subagent / 多 Agent |
| **gemini-cli** | 配置层层级（default→global→workspace→env→CLI）、Shell 命令安全解析（红irection 检测、命令链阻断）、Sandbox Mode TOML profile | 10.12 Release门槛 / shell safety；近期 config 层增强 |
| **oh-my-openagent** | Team Mode 多 Agent 协作（Lead+Member，最多 8 人）、Delegate Core 模型选择 fallback 链、Skills Loader 文件系统 + 共享 | 10.10 Skills/Hooks；中远期多 Agent |

#### 跨项目高频模式整理

以下模式在至少 3 个参考项目中以相似的方式实现，说明其已经是行业共识：

**1. 工具权限三层规则集**
CodeWhale（BuiltinDefault / Agent / User）、OpenCode（Policy scope）、gemini-cli（sandbox mode TOML）、DeepSeek-Reasonix（permission.Policy + agent.Gate）。
→ Joker 当前有两层（`PermissionEngine` + `SharedApprovalChannel`），但缺少"用户可配置的持久化规则集"。应改为三层：BuiltinDefault → FileConfig → SessionGrant，且规则应允许 tool+command+path 三级模式匹配（参考 CodeWhale 的 `ToolAskRule`）。

**2. MCP 工具命名与 Server Scope**
CodeWhale（`mcp__server__tool`）、OpenCode（MCP 工具注册带 server 上下文）、claude-code（MCP Manager 的 `serverName + toolName` 双 key 引用）。
→ Joker 当前只用了 `mcp_{tool}`，没有 server 命名空间。应改为 `mcp__{server}__{tool}`。

**3. LLM 驱动的上下文压缩**
pi（LLM summary + file operation tracking）、OpenCode（compaction epoch + projector replay）、gemini-cli（ChatCompressionService）。
→ Joker 当前是 heuristic summary，应接入真正的 LLM 摘要，并追踪压缩段内读取/修改的文件（参考 pi 的 `computeFileLists`）。

**4. 配置层层级**
gemini-cli（default→global→workspace→env→CLI）、OpenCode（Directory Snapshot 聚合 5 个源头）、CodeWhale（config crate 合并 TOML + CLI + 环境变量）。
→ Joker 当前是 CLI flags > joker.toml > built-in defaults，缺少 global config 层和 workspace 覆盖层。

**5. 工具执行并发控制**
CodeWhale（`ToolCallRuntime` 读写锁）、pi（并行执行+sequential fallback）、OpenCode（Semaphore 控制的并发工具槽）。
→ Joker 已有 `ParallelWhenSafe`，但缺少 CodeWhale 那样的读锁/写锁细分（parallel-safe 工具不能互相写干扰）。

**6. 事件驱动 UI 渲染**
pi（EventStream push 模型）、OpenCode（EventV2 + Projector）、DeepSeek-Reasonix（Controller → Sink）。
→ Joker 当前已有 `Observer` 和 `Event` 枚举，方向正确，但尚未使用事件溯源存储。

#### 本章结构映射

```
参考模式                → 对应章节
───────                ─────────
会话树                 → 10.4 P0 Session + 10.9 P2 事件溯源
ToolCapability 枚举     → 10.8 P1 工具增强
三层规则集              → 10.6 P1 权限收敛
MCP qualified name     → 10.11 P2 MCP 完整化
LLM 压缩               → 10.4 P0 Session + 9.11 后续改进
配置层层级              → 10.14 (本调研新增)
Shell 安全 / 命令链     → 10.14 (本调研新增)
Controller 模式        → 10.5 P1 TUI 驱动
多 Agent 编排          → 10.14 (本调研新增)
工作流引擎              → 长期可选项
事件溯源               → 10.9 P2
Skills/Hooks 分阶段    → 10.10 P2
Checkpoint/Time Machine→ 中远期
```

## 10. 后续实现路线与不变量

本章不是愿望清单，而是从当前代码状态推导出的具体工程路线。每项都尽量说明“为什么现在该做、应该改哪里、验收标准是什么”。

### 10.1 必须保持的架构不变量

1. `joker` core 不依赖 TUI、config、provider、tools、MCP。
2. Provider 特例只存在于 `joker-provider`，通过 `ModelResponseEvent` 回到 core。
3. 工具只声明能力和执行逻辑，不直接询问用户。
4. 权限判断先决定 tool visibility，再决定 invocation approval。
5. 文件工具必须通过 `WorkspaceTool` 做路径解析。
6. TUI 只是一个宿主；未来 CLI one-shot / HTTP / ACP / MCP server 应复用 core 装配链。
7. Session/message/event 不要混在 UI transcript item 中，UI transcript 是投影，不是事实源。
8. Shell 沙箱/安全检测不在 `joker-tools` 的 `shell` 实现中做策略决策；策略层通过 `ChainPolicy` 决定（参考 CodeWhale 的 ExecPolicyEngine 分层 + gemini-cli 的 shell safety 独立 parser）。
9. 会话树不等于 transcript 投影：会话存储应保留完整事件历史（参考 OpenCode 的 EventV2 + Projector），TUI transcript 只是临时 UI 状态。
10. 传输无关的会话控制层：`AgentRuntime` 的职责应发展为类似 DeepSeek-Reasonix Controller 的独立编排层，TUI、CLI one-shot、HTTP server 都通过同一控制接口通信。

### 10.2 P0：修正文档与代码不一致的用户路径

当前 README 几乎为空，`docs/config.md` 仍描述 `/config` 命令，`OUTLINE.md` 早期章节仍说“当前先只支持 DeepSeek”。应做一次 docs sync：

- README 写最小可运行路径。
- `docs/providers.md` 更新 provider profiles 和 API key 输入机制。
- `docs/config.md` 删除或标记未实现 `/config` 命令。
- OUTLINE 第 1-8 章可以保留为历史蓝图，但需要在每章开头标注“规划/历史”或迁移为现状版。

验收：

- 新用户按 README 能跑 scripted provider。
- DeepSeek / Anthropic / Google 配置示例能对应当前 CLI/TUI。
- 文档不再把未实现命令写成已实现。

### 10.3 P0：让 custom agent 真正可重启加载

当前 `/agent new` 可以写 `joker.toml` 和 constraint file，但启动加载链不完整。建议：

1. 修改 `RuntimeConfig`，保留 resolved agent configs 和 mcp server configs；或新增 `LoadedConfig { runtime, file }`，让 TUI 同时持有二者。
2. `AgentDriver::new_with_agents_dir` 从真实 config 注册 custom agents。
3. `assemble_system_prompt` 支持读取 constraint file 内容，内置 agent 只作为首次生成默认文件。
4. `App::with_config` 的 `agent_names` 来自 driver/permission engine，而不是硬编码 plan/build/yolo。
5. 与 10.5 配合：custom agent 的重载机制应设计为 `AgentRuntime` 的 SwitchAgent Op 的一部分，而不是绕过 runtime 直接改 TUI 状态（参考 DeepSeek-Reasonix Controller 的 transport-agnostic 设计：前端只发命令，Controller 负责切换和编排）。

验收：

- `/agent new` 创建 agent，退出重启后 `/agent list` 能看到它。
- 修改 `~/.joker/agents/foo_agent.md` 后，foo agent 下一次 run 的 system prompt 包含修改内容。
- custom agent 的 tool permissions 在 materialize 后生效。
- `/agent switch <name>` 在运行中通过 Op 切换，不在前端复制 agent 状态。

### 10.4 P0：明确 session 保存链路

`JsonlSessionStore` 已实现，且 TUI 在 run 完成后会调用 `save_current_session` 保存。当前缺口不是"是否保存"，而是：

- Run started 时创建 session id/label，支持从 loaded session 继承 parent_id/root_id。
- Run completed 时保存 final conversation；如果 conversation 来自 loaded session，应 fork 或原地更新。
- `/sessions load` 后下一次提交应继续该 conversation，并保存回同一 id 或 fork。
- 长期应迁移到 pi 的**会话树模型**：`message` | `compaction` | `branch_summary` | `model_change` | `thinking_level_change` 等条目类型，树状结构而非线性 JSONL。当前 JSONL 可以作为中间存储。
- 压缩阶段应追踪文件操作（参考 pi 的 `computeFileLists`：从 tool result 中提取 readFiles/modifiedFiles），在 LLM summary 中保留文件变更时间线。

验收：

- 发一条 scripted prompt，退出重启，`/sessions list` 可见。
- `/sessions load <id>` 后 transcript 和 conversation 都恢复。
- 继续对话后 session 有正确的 parent_id/root_id 继承。
- 压缩后有 read/modified files 摘要信息。

### 10.5 P1：把 TUI 驱动切到 `AgentRuntime`

当前 core 已有 `AgentRuntime`，但 TUI 直接 spawn `Agent::run`。建议逐步切换：

- `AgentDriver` 构造 `AgentRuntime`。
- TUI 保持 `mpsc<Op>` handle。
- Cancel/Approve/Compact/SwitchAgent 走 Op。
- Interrupt/steer 在运行中可用。

验收：

- 运行中 `Ctrl-C` 通过 `Op::Cancel` 生效。
- approval 通过 `Op::Approve` 生效。
- `/compact` 在 active runtime 中可触发真实 compaction 或明确返回“下轮生效”。
- 运行中 steer 消息能进入下一 turn。

### 10.6 P1：权限系统收敛

当前 session grants 同时存在于 `PermissionEngine` 和 `SharedApprovalChannel`。建议：

- 选择一个事实源。更合理的是 `PermissionEngine` 负责 grants，approval channel 只负责传递 UI response。
- 引入三层规则集（参考 CodeWhale 的 `ExecPolicyEngine`：BuiltinDefault → Agent → User），用户层配置写入 `joker.toml` 或 `~/.joker/config.toml`。
- 规则应支持 tool+command+path 三级模式匹配（参考 CodeWhale 的 `ToolAskRule`：每个规则有 tool 名、可选命令前缀、可选路径模式、action 四字段）。
- 引入 `ToolCapability` 类型代替单一的 `mutating: bool`（参考 CodeWhale 的 `ToolCapability` 枚举：ReadOnly / WritesFiles / ExecutesCode / Network / Sandboxable / RequiresApproval），metadata 更丰富，策略层可据此做更细致判断。
- `ApprovalResponse::Approved { remember_for_session }` 后调用 engine grant。
- TUI 中持有 engine mutable handle 或通过 runtime op 回调 grant。
- 对 MCP 工具默认策略统一：建议 unknown/mcp network tools 默认 Ask。
- 修复 plan agent `plans/*.md` 例外不可见的问题。

验收：

- Allow for Session 后同 agent 同 tool 不再弹窗。
- 重启是否保留由明确配置决定（BuiltinDefault 层提供默认，User 层覆盖）。
- plan agent 能按设计写计划文件，或文档明确 plan 完全只读。
- MCP 工具不会默认无提示执行未知外部动作。

### 10.7 P1：Provider 质量提升

短期优先：

- request body golden tests。
- SSE parser golden tests，覆盖 text、reasoning、single tool、multiple tools、usage、finish reason。
- `discover_models` 对 Anthropic/Google 的模型列表端点策略单独处理，不要只套 OpenAI `/models`。
- `Route::build_model_for` 对 missing key 的错误文案指向 TUI API key 输入或 env var。

中期：

- provider error taxonomy：Auth、RateLimited、Quota、ModelNotFound、ContextLength、Network、Protocol。
- usage/caching extraction。
- provider-specific max tokens / context window catalog。
- model capability discovery：tools、reasoning、vision、streaming。

验收：

- DeepSeek/OpenAI-compatible、Anthropic、Google 至少各有 request snapshot。
- 错误能在 TUI 中显示可操作信息。
- `/model` 下拉来自 discovery，而不是只有当前 model。

### 10.8 P1：工具增强

按风险排序：

- `write_file` 增加 stale check、newline/BOM preservation、diff preview。
- `apply_patch` 改成完整多文件 patch parser，或重命名当前工具为 `apply_unified_patch_to_file` 以避免误导。
- `shell` 增加 job status/kill 工具，或移除 `bg` 直到有管理能力。
- `fetch_url` 换成更可靠的 HTML to Markdown/readability。
- `list_files` 是否尊重 `.gitignore` 需要产品决策，并与 `glob` 对齐。
- 引入 `ToolCapability` 枚举替代单一的 `mutating: bool`（与 10.6 配合），让工具 metadata 可以表达 ExecutesCode / Network / Sandboxable 等语义。
- 工具执行并发控制参考 CodeWhale 的 `ToolCallRuntime`：用 `RwLock` 实现读锁（parallel-safe 工具可重叠）和写锁（mutating 工具排他），重入调用跳过锁检测。Joker 当前仅靠 `AgentConfig.execution_mode: Sequential | ParallelWhenSafe`，缺少细粒度锁层级。

验收：

- 文件写入/编辑工具都有 workspace escape tests。
- Patch 失败能指出具体 hunk。
- Shell timeout 后不会留下子进程。
- Web fetch 不允许 private IP 和 localhost 域名绕过。
- 并行执行的 parallel-safe 工具不会在写入时互相干扰。

### 10.9 P2：事件溯源与 UI 投影

当前 `Event` 很丰富，但 session store 是 snapshot。后续如果要支持恢复、审计、fork、TUI replay，建议引入事件日志：

- 每次 run/turn/tool/approval/model delta 追加 event。
- conversation 是事件投影。
- TUI transcript 是 conversation + event 的 UI 投影。
- JSONL 可先作为 event log，不急于 SQLite。

验收：

- 崩溃后能从事件日志恢复到最后完整 turn。
- tool call 和 approval 有可审计记录。
- session fork 可以基于某个 event/message id，而不是只 fork 整个 snapshot。

### 10.10 P2：Skills / Hooks 落地

当前有 trait 和 registry，但未接入 runtime。落地顺序：

1. 定义 skill 文件格式和加载目录：repo `.joker/skills`、user `~/.joker/skills`。参考 oh-my-openagent 的 `skills-loader-core`：扫描目录、解析 markdown skill 文件、注册到 skill registry、注入 agent prompt。
2. `SkillRegistry` 在 driver 初始化时加载。
3. context build 前根据 workspace/files 激活 skills。
4. `allowed_tools` 与 `PermissionEngine::materialize_tools` 取交集。
5. HookRegistry 接入 `Agent::run_turn` 和 `execute_tool_call`。参考 pi 的 three-phase tool execution 结构：
   - `prepareToolCall()`：参数校验 + schema 验证 + `beforeToolCall()` -> 返回 PreparedToolCall 或 ImmediateToolCallOutcome (blocked/error)
   - `executePreparedToolCall()`：实际执行
   - `finalizeExecutedToolCall()`：`afterToolCall()` 后处理/修改结果
6. `Hook` 的执行顺序参考 oh-my-openagent 的 hook chain：多个 hook 按注册顺序依次执行，允许任一 hook 阻断后续执行。

验收：

- 一个 Rust skill 可以对 `*.rs` 注入 prompt。
- skill 限制工具后，模型看不到被限制工具。
- before_tool_call hook 能阻止调用并返回 tool error。
- after_tool_call hook 能修改工具输出。

### 10.11 P2：MCP 完整化

建议路线：

- 工具命名改为 `mcp__{server}__{tool}`，总长度限制 64 字符，超长时使用 hash 截断（参考 CodeWhale 的 qualified name 实现：`crates/mcp/src/lib.rs` 的 `parse_qualified_tool_name` + 64-char truncation + hash）。
- config 中的 server name 进入 adapter，每个 server 维护独立的 `McpManagedClient`（参考 CodeWhale 的 `McpManager`：持有 server 配置与客户端实例的映射）。
- 支持 `toolsChanged` 事件通知，server 重启后自动重新 discover 工具列表（参考 claude-code MCP Manager 的事件系统：`connected` / `disconnected` / `toolsChanged` / `error` / `authRequired`）。
- 支持自动重连：断线后按指数退避尝试重连，重连成功后触发 `toolsChanged`。
- 增加 streamable HTTP/SSE transport。当前只实现 stdio。
- MCP tool metadata 加 mutating/network/approval hints，如果 server 没提供则默认 Ask。
- 可选：增加 MCP server 认证支持（参考 claude-code 的 `McpAuthTool` + 服务器端 OAuth 授权码流程）。

验收：

- 两个 MCP server 有同名 tool 时不冲突（由 `mcp__{server}__{tool}` 区分）。
- server 重启后工具能通过 `toolsChanged` 事件重新 discover。
- TUI `/agent` 权限向导能显示 MCP tools（含 server 分组）。
- 连接断开后自动重连，不丢失已注册的 tool list。

### 10.12 Release 前最低质量门槛

在把 Joker 当成可日常使用工具前，至少满足：

- `cargo test --workspace` 通过。
- README 可按 scripted provider 跑通。
- 一个真实 OpenAI-compatible provider 跑通 text + tool call。
- build agent 修改文件前会审批。
- yolo agent 自动执行但 shell 危险命令仍被安全策略拦截。
- plan agent 的只读边界有测试。
- session 能保存和恢复。
- API key 不写入 `joker.toml`。
- workspace escape tests 覆盖 read/write/edit/patch。

### 10.13 开发任务拆分建议

短小任务优先按 crate 拆：

1. `joker-config`: Preserve agent/mcp configs in RuntimeConfig.
2. `joker-tui`: Load custom agents on startup and read constraint files.
3. `joker-tui`: Save sessions after run completion (include parent_id/root_id inheritance).
4. `joker`: Unify session grants through PermissionEngine.
5. `joker`: Add ToolCapability enum replacing bare mutating: bool.
6. `joker-tools`: Add write_file stale/newline/BOM behavior.
7. `joker-tools`: Add ToolCallRuntime with RwLock for concurrent tool execution.
8. `joker-provider`: Add request body golden tests.
9. `joker-mcp`: Rename MCP tools to mcp__{server}__{tool} + add reconnect.
10. `joker-tui`: Migrate driver from direct Agent::run to AgentRuntime.
11. `joker-tui`: Add shell safety ChainPolicy (command chain detection, redirection).
12. `docs`: Sync README/docs with current commands.

中后期任务：

13. Swap JSONL session store to pi-style session tree model.
14. LLM-based compaction with file operation tracking.
15. Config layer hierarchy: built-in → ~/.joker/config.toml → ./.joker/config.toml → CLI flags.
16. Define and load Skills file format (.joker/skills/*.md).
17. Wire HookRegistry into Agent::run_turn as three-phase lifecycle.

每个任务的验收都应有至少一个测试或 smoke path，避免只改文档或只改实现造成再次漂移。
### 10.14 P2+：新路线项（本调研新增）

以下路线项来自本章调研的 8 个参考项目的交叉验证，不属于短期必须，但长期决定 Joker 的完整性和可维护性。

#### A. Shell 命令安全体系

当前 `shell` 工具在 `joker-tools` 中只有基础检测（blocked env vars、chaining 警告），没有主动阻断。参考 gemini-cli 的 shell safety + CodeWhale 的 ExecPolicyEngine：

- 引入 shell 命令解析层：检测命令链（`;` `&&` `||` `|`）、重定向、注入模式。命令链中任一段匹配 deny 规则则整体拒绝。
- 引入 arity-aware 前缀匹配（参考 CodeWhale 的 `BashArityDict`）：“git log” 与 “git” 区分对待，避免因 “git” 被信任而误放行 “git rm -rf”。
- 策略层新增 `ChainPolicy` 规则类型：trusted_prefixes（信任命令前缀）/ denied_prefixes（拒绝命令前缀）/ ask_rules（tool + command + path 三级匹配）。
- 验收：`; rm -rf /` 在任何可执行 shell 工具中被阻断；`git log ; echo ok` 不被信任；`git push origin main --force` 触发询问而不是被 `git` 全局信任。

#### B. 配置层层级

当前只有 CLI flags > joker.toml > built-in defaults。缺少 global 层和 workspace 覆盖层。参考 gemini-cli 的五层模型：

- 新增 `~/.joker/config.toml` 全局配置层（低于 CLI flags、高于 joker.toml）。
- 新增 workspace-override 层：`.joker/config.toml`（覆盖全局但不覆盖 CLI flags）。
- 完成后层级：CLI flags > workspace `.joker/config.toml` > `~/.joker/config.toml` > 项目 `joker.toml` > built-in defaults。
- 每层可配置项：model、provider、agent policy overrides、mcp servers、tool permissions、sandbox mode。
- 参考 OpenCode 的 Directory Snapshot：启动时并发加载 providers + agents + commands + skills + config 并聚合成一个 snapshot。

#### C. 多 Agent 编排方向（远期）

当前 Agent 系统只有单 Agent run + AgentRuntime，没有嵌套 subagent 或多 Agent 协作。参考 DeepSeek-Reasonix 的 subagent 深度控制 + oh-my-openagent 的 Team Mode：

- 允许 Agent 工具创建一个 SubAgent 并获取其输出（类似 claude-code 的 `AgentTool`）。
- 支持 subagent 深度限制（参考 DeepSeek-Reasonix 的 `DefaultMaxSubagentDepth = 2`）。
- 中远期可引入 Team Mode：Lead Agent + 多个 Member Agent，通过 mailbox 通信（参考 oh-my-openagent 的 `team-mailbox.ts` + `team-tasklist.ts`）。

#### D. 工作流引擎（远期）

参考 claude-code 的 Workflow Engine + OpenCode 的 Scripted Workflow：

- 定义确定性脚本（JSON/YAML），描述多步骤 agent 调用序列。
- 支持结构化输出 schema 验证、token 预算控制、断点续传。
- 不需要立即实现，但在 agent server 扩展时应预留 `journal` / `budget` / `structured output` 等概念。
