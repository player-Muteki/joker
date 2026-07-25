# Joker 项目总纲

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

