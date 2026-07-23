# Providers

Joker keeps provider code outside the core agent kernel. The core `joker` crate
only depends on the `Model` trait; provider crates implement that trait.

## Scripted

`scripted` is the default local provider. It requires no network and no API key.

```bash
cargo run -p joker-tui -- --provider scripted
```

## DeepSeek

DeepSeek uses the OpenAI-compatible adapter.

Defaults:

```text
provider = deepseek
base_url = https://api.deepseek.com
api_key_env = DEEPSEEK_API_KEY
model = deepseek-v4-flash
models = deepseek-v4-flash, deepseek-v4-pro
```

Run:

```bash
export DEEPSEEK_API_KEY=sk-...
cargo run -p joker-tui -- --provider deepseek --model deepseek-v4-flash
```

Inside the TUI:

```text
/provider deepseek
/model deepseek-v4-pro
```

## OpenAI-Compatible

Use this for local gateways, OpenRouter-style endpoints, vLLM, Ollama-compatible
OpenAI routes, and other providers that expose `/chat/completions`.

```bash
cargo run -p joker-tui -- \
  --provider openai-compatible \
  --base-url http://localhost:8000/v1 \
  --model qwen \
  --api-key-env LOCAL_LLM_API_KEY
```

Inside the TUI:

```text
/provider openai-compatible
/config set base_url http://localhost:8000/v1
/config set api_key_env LOCAL_LLM_API_KEY
/model qwen
```
