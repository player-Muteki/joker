# Joker

Joker is an API-first Rust coding agent. It can run as an interactive terminal
UI for daily development or as a headless command for scripts, CI jobs, and
automation.

## What Works

- Interactive TUI with streaming assistant output, slash commands, sessions,
  provider/model switching, and approval prompts.
- Headless `exec` mode that runs one prompt and prints the final assistant
  response.
- Project-local `joker.toml` configuration with CLI overrides.
- Built-in providers for scripted local runs, DeepSeek, Anthropic, Google,
  Alibaba/DashScope, ZhipuAI, Moonshot/Kimi, Baidu/ERNIE, and custom
  OpenAI-compatible endpoints.
- Workspace-scoped tools for file reads, search, edits, patching, shell
  commands, memory, todos, URL fetches, and MCP-discovered tools.
- Agent profiles (`plan`, `build`, `yolo`) with different permission postures.

## Install

From this repository:

```bash
cargo install --path crates/joker-tui
```

For development, run the binary directly:

```bash
cargo run -p joker-tui --bin joker
```

## Quick Start

Create a project config:

```bash
joker init
```

Run the interactive TUI:

```bash
joker
```

Start the TUI with an initial prompt:

```bash
joker --prompt "review this repository"
```

Run one non-interactive prompt:

```bash
joker exec "summarize this repository" --provider scripted
```

Print the config path that this invocation will use:

```bash
joker config path
```

## Providers

`scripted` is the default provider. It requires no network and is useful for
smoke tests:

```bash
joker exec "hello" --provider scripted --scripted-response "Joker is ready."
```

Use a hosted provider by exporting its API key:

```bash
export DEEPSEEK_API_KEY=sk-...
joker --provider deepseek --model deepseek-chat
```

Use a custom OpenAI-compatible endpoint:

```bash
joker --provider openai-compatible \
  --base-url http://localhost:8000/v1 \
  --model qwen \
  --api-key-env LOCAL_LLM_API_KEY
```

Non-interactive `exec` fails early if the selected provider needs an API key
that is not present in the environment.

## Configuration

Joker reads `joker.toml` in the current directory by default:

```toml
provider = "deepseek"
model = "deepseek-chat"
demo_tool = false

[agent.build.tools.read_file]
permission = "auto-accept"

[agent.build.tools.write_file]
permission = "ask"
```

CLI flags override the file for a single invocation:

```bash
joker --config ./configs/joker.toml --provider scripted
```

Secrets are not stored in `joker.toml`; providers read credentials from
environment variables such as `DEEPSEEK_API_KEY`, `ANTHROPIC_API_KEY`, or a
custom `--api-key-env`.

## Safety Model

The default `build` agent can inspect files automatically and asks before
mutating files or running shell commands. The `plan` agent is read-only for
mutating tools. The `yolo` agent auto-accepts more operations and should only
be used in trusted workspaces.

Headless `exec` is conservative: if a tool requires approval, Joker denies that
tool request instead of blocking for user input.

## Development

Run the test suite:

```bash
cargo test -q
```

Run lint checks:

```bash
cargo clippy -q --all-targets
```

Format Rust code:

```bash
cargo fmt
```
