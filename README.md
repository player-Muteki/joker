# Joker

Joker is a small, customizable Rust TUI agent.

The product shape is intentionally simple:

- `joker`: minimal agent kernel traits and orchestration.
- `joker-provider-openai`: OpenAI-compatible streaming model adapter.
- `joker-config`: project config and provider presets.
- `joker-tools`: read-only workspace tools.
- `joker-tui`: terminal host app with slash commands.

## Quickstart

Run local scripted mode without any API key:

```bash
cargo run -p joker-tui -- --provider scripted
```

Run one prompt in scripted mode:

```bash
cargo run -p joker-tui -- --prompt "hello"
```

Run DeepSeek:

```bash
export DEEPSEEK_API_KEY=sk-...
cargo run -p joker-tui -- --provider deepseek --model deepseek-v4-flash
```

The product binary is also available as `joker`:

```bash
cargo run -p joker-tui --bin joker -- --provider scripted
```

## Slash Commands

Inside the TUI:

```text
/help
/status
/provider
/provider deepseek
/provider scripted
/model
/model deepseek-v4-flash
/models
/config show
/config set provider deepseek
/config set model deepseek-v4-flash
/config save
/tools
/clear
/cancel
/quit
```

Typing `/` opens a small command suggestion panel.

## Config

Joker reads `joker.toml` from the current project by default. CLI flags override
the file for the current launch. Slash command changes are session-local until
you run `/config save`.

See [docs/config.md](docs/config.md) and [docs/providers.md](docs/providers.md).

## Verification

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo doc --workspace --no-deps
```
