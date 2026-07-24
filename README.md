# Joker

Joker is a small, customizable Rust TUI agent.

The product shape is intentionally simple:

- `joker`: minimal agent kernel traits and orchestration.
- `joker-provider-openai`: OpenAI-compatible streaming model adapter.
- `joker-config`: project config and provider presets.
- `joker-tools`: read-only workspace tools.
- `joker-tui`: terminal host app with slash commands.
