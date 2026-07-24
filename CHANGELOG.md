# Changelog

## 0.1.0

- **PermissionPolicy + 审批通道**：ToolDecision::Ask 支持调用时暂停等待用户批准，SharedApprovalChannel 桥接 agent 与 TUI/API。
- **AgentBuilder / ToolSet 友好 API**：fluent builder 装配 agent，支持 .read()/.grep()/.write()/.shell()/.web_search() 组合。
- **写入/Shell 工具**：write_file、edit_file、apply_patch（unified diff）、shell 命令执行含安全分析（命令链检测、LD_PRELOAD阻断、trusted command prefix）。
- **WebSearch / FetchURL 工具**：WebSearch trait + DuckDuckGoSearch 后端（免 API key）、fetch_url（私有 IP 阻断、HTML 提取）。
- **记忆工具**：memory_read / memory_write，文件存储 .joker-memory/。
- **会话持久化**：SessionStore trait + JsonlSessionStore 实现。
- **SummaryContextBuilder**：长会话自动摘要预置到上下文。
- **配置扩展**：TOML 支持 agent profile、tool permission、规则配置（AgentProfileConfig、PermissionRuleConfig）。
- **Slash command 扩展**：/approve、/deny（支持 --session 标记）、/sessions（连接实际 SessionStore）、/compact（触发 SummaryContextBuilder）。
- **集成测试**：64 个测试覆盖 agent loop、权限审批、session 持久化、写入工具、网络工具。

## 0.0.1

- Added Rust TUI product binary with streaming transcript rendering.
- Added slash commands for help, status, provider/model switching, config, tools,
  clearing, canceling, and quitting.
- Added OpenAI-compatible provider adapter with DeepSeek preset.
- Added project-local config loading/saving through `joker.toml`.
- Added read-only workspace tools: `list_files`, `read_file`, and `grep`.
- Kept the core `joker` crate free of TUI, HTTP, config, and provider code.
