# Joker Config

Joker 0.0.1 reads project-local `joker.toml` by default.

Priority:

```text
CLI flags
> joker.toml
> built-in defaults
```

Secrets are not stored in `joker.toml`. Provider API keys are read from env vars.

## Example

```toml
provider = "deepseek"
model = "deepseek-v4-flash"
scripted_response = "Hello from Joker TUI."
demo_tool = false
```

Custom OpenAI-compatible provider:

```toml
provider = "local"
model = "qwen"

[providers.local]
kind = "openai"
base_url = "http://localhost:8000/v1"
model = "qwen"
api_key_env = "LOCAL_LLM_API_KEY"
```

## TUI Commands

Config commands apply to the running session first:

```text
/config show
/config set provider deepseek
/config set model deepseek-v4-flash
/config set base_url http://localhost:8000/v1
/config set api_key_env LOCAL_LLM_API_KEY
/config save
```

Only `/config save` writes `joker.toml`.
