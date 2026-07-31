//! CLI argument parsing and binary entry point.
//!
//! The binary exposes the interactive TUI by default, plus product-oriented
//! subcommands for headless execution and first-run configuration.

use std::{
    fs,
    io::{self, ErrorKind, Write},
    path::{Path, PathBuf},
};

use clap::{Args, Parser, Subcommand};
use joker_config::{ConfigOverrides, ConfigStore, RuntimeConfig};

use crate::{TuiError, TuiOptions, driver::AgentDriver, run_tui};

const DEFAULT_CONFIG_TEMPLATE: &str = r#"# Joker project configuration.
# Secrets are read from environment variables and are never stored here.

provider = "scripted"
model = "scripted"
scripted_response = "Hello from Joker."
demo_tool = false

# Common provider choices:
# provider = "deepseek"
# model = "deepseek-chat"
# export DEEPSEEK_API_KEY=sk-...
#
# provider = "anthropic"
# model = "claude-sonnet-4-20250514"
# export ANTHROPIC_API_KEY=sk-ant-...
#
# provider = "openai-compatible"
# base_url = "http://localhost:8000/v1"
# model = "model"
# api_key_env = "OPENAI_COMPATIBLE_API_KEY"

# Built-in agents are: plan, build, yolo.
default_agent = "build"

[agent.build.tools.read_file]
permission = "auto-accept"

[agent.build.tools.list_files]
permission = "auto-accept"

[agent.build.tools.grep]
permission = "auto-accept"

[agent.build.tools.write_file]
permission = "ask"

[agent.build.tools.apply_patch]
permission = "ask"

[agent.build.tools.shell]
permission = "ask"
"#;

/// CLI argument structure for the `joker` binary.
#[derive(Debug, Parser)]
#[command(
    name = "joker",
    version,
    about = "Joker coding agent",
    long_about = "Joker runs an AI coding agent in a terminal UI or as a non-interactive command."
)]
pub struct Cli {
    #[command(flatten)]
    flags: RuntimeFlags,
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long)]
    no_alt_screen: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Clone, Debug, Args)]
struct RuntimeFlags {
    #[arg(long, global = true, default_value = "joker.toml")]
    config: PathBuf,
    #[arg(long, global = true)]
    provider: Option<String>,
    #[arg(long, global = true)]
    model: Option<String>,
    #[arg(long, global = true)]
    base_url: Option<String>,
    #[arg(long, global = true)]
    api_key_env: Option<String>,
    #[arg(long, global = true)]
    scripted_response: Option<String>,
    #[arg(long, global = true)]
    demo_tool: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run one prompt non-interactively and print the final assistant text.
    Exec(ExecArgs),
    /// Create a starter joker.toml in the current project.
    Init(InitArgs),
    /// Inspect effective CLI configuration paths.
    Config(ConfigArgs),
}

#[derive(Debug, Args)]
struct ExecArgs {
    /// Agent profile to use for the run.
    #[arg(long)]
    agent: Option<String>,
    /// Prompt to send to the agent.
    #[arg(value_name = "PROMPT", required = true, num_args = 1..)]
    prompt: Vec<String>,
}

impl ExecArgs {
    fn prompt_text(&self) -> String {
        self.prompt.join(" ")
    }
}

#[derive(Debug, Args)]
struct InitArgs {
    /// Replace an existing config file.
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct ConfigArgs {
    #[command(subcommand)]
    command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Print the config path used by this invocation.
    Path,
}

/// Parse CLI args and run the selected command.
pub async fn run() -> Result<(), TuiError> {
    let cli = Cli::parse();
    match &cli.command {
        Some(Command::Exec(args)) => run_exec(&cli.flags, args).await,
        Some(Command::Init(args)) => run_init(&cli.flags, args),
        Some(Command::Config(args)) => run_config(&cli.flags, args),
        None => run_interactive(cli).await,
    }
}

async fn run_interactive(cli: Cli) -> Result<(), TuiError> {
    let (config_store, runtime_config) = load_runtime_config(&cli.flags)?;
    run_tui(TuiOptions {
        initial_prompt: cli.prompt,
        use_alt_screen: !cli.no_alt_screen,
        config_store,
        runtime_config,
    })
    .await
}

async fn run_exec(flags: &RuntimeFlags, args: &ExecArgs) -> Result<(), TuiError> {
    let (_config_store, runtime_config) = load_runtime_config(flags)?;
    validate_headless_config(&runtime_config)?;

    let workspace = std::env::current_dir()?;
    let agents_dir = joker_home_dir().join("agents");
    let mut driver = AgentDriver::new_with_agents_dir(runtime_config, workspace, agents_dir);
    if let Some(agent) = &args.agent {
        driver.set_active_agent(agent.clone());
    }
    driver.init_mcp_servers().await;

    let outcome = driver.run_headless(args.prompt_text()).await?;
    if !outcome.assistant_text.is_empty() {
        write_stdout_line(&outcome.assistant_text)?;
    }
    Ok(())
}

fn run_init(flags: &RuntimeFlags, args: &InitArgs) -> Result<(), TuiError> {
    create_config_file(&flags.config, args.force)?;
    write_stdout_line(&format!(
        "Created {}",
        display_config_path(&flags.config)?.display()
    ))
}

fn run_config(flags: &RuntimeFlags, args: &ConfigArgs) -> Result<(), TuiError> {
    match args.command {
        ConfigCommand::Path => {
            write_stdout_line(&display_config_path(&flags.config)?.to_string_lossy())
        }
    }
}

fn load_runtime_config(flags: &RuntimeFlags) -> Result<(ConfigStore, RuntimeConfig), TuiError> {
    let config_store = ConfigStore::new(flags.config.clone());
    let runtime_config = config_store.load(ConfigOverrides {
        provider: flags.provider.clone(),
        model: flags.model.clone(),
        base_url: flags.base_url.clone(),
        api_key_env: flags.api_key_env.clone(),
        scripted_response: flags.scripted_response.clone(),
        demo_tool: flags.demo_tool.then_some(true),
    })?;
    Ok((config_store, runtime_config))
}

fn validate_headless_config(runtime_config: &RuntimeConfig) -> Result<(), TuiError> {
    if let Some(env_var) = runtime_config.needs_api_key() {
        return Err(TuiError::Cli(format!(
            "provider requires API key env var {env_var}; export it or use --provider scripted"
        )));
    }
    Ok(())
}

fn create_config_file(path: &Path, force: bool) -> Result<(), TuiError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    if force {
        fs::write(path, DEFAULT_CONFIG_TEMPLATE)?;
        return Ok(());
    }

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            if error.kind() == ErrorKind::AlreadyExists {
                TuiError::Cli(format!(
                    "{} already exists; pass --force to replace it",
                    path.display()
                ))
            } else {
                TuiError::Io(error)
            }
        })?;
    file.write_all(DEFAULT_CONFIG_TEMPLATE.as_bytes())?;
    Ok(())
}

fn display_config_path(path: &Path) -> Result<PathBuf, TuiError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()?.join(path))
}

fn write_stdout_line(line: &str) -> Result<(), TuiError> {
    write_stdout(line)?;
    write_stdout("\n")
}

fn write_stdout(text: &str) -> Result<(), TuiError> {
    let mut stdout = io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|_| stdout.flush())
    {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(TuiError::Io(error)),
    }
}

fn joker_home_dir() -> PathBuf {
    std::env::var_os("JOKER_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".joker"))
        })
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(".joker")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_prompt_joins_unquoted_words() {
        let cli = Cli::parse_from(["joker", "exec", "fix", "the", "tests"]);
        let Some(Command::Exec(args)) = cli.command else {
            panic!("expected exec command");
        };
        assert_eq!(args.prompt_text(), "fix the tests");
    }

    #[test]
    fn init_refuses_to_overwrite_without_force() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("joker.toml");
        fs::write(&path, "provider = \"scripted\"\n").unwrap();

        let result = create_config_file(&path, false);

        assert!(matches!(result, Err(TuiError::Cli(message)) if message.contains("--force")));
    }

    #[test]
    fn init_writes_product_ready_template() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("joker.toml");

        create_config_file(&path, false).unwrap();
        let raw = fs::read_to_string(path).unwrap();

        assert!(raw.contains("provider = \"scripted\""));
        assert!(raw.contains("[agent.build.tools.write_file]"));
        assert!(raw.contains("permission = \"ask\""));
    }
}
