# Changelog

## 0.0.1

- Added Rust TUI product binary with streaming transcript rendering.
- Added slash commands for help, status, provider/model switching, config, tools,
  clearing, canceling, and quitting.
- Added OpenAI-compatible provider adapter with DeepSeek preset.
- Added project-local config loading/saving through `joker.toml`.
- Added read-only workspace tools: `list_files`, `read_file`, and `grep`.
- Kept the core `joker` crate free of TUI, HTTP, config, and provider code.
