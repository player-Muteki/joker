use clap::Parser;
use joker_config::{ConfigOverrides, ConfigStore};

use crate::{TuiOptions, run_tui};

#[derive(Debug, Parser)]
#[command(name = "joker", about = "Terminal UI for Joker agent kernel")]
pub struct Cli {
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long)]
    no_alt_screen: bool,
    #[arg(long, default_value = "joker.toml")]
    config: std::path::PathBuf,
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    base_url: Option<String>,
    #[arg(long)]
    api_key_env: Option<String>,
    #[arg(long)]
    scripted_response: Option<String>,
    #[arg(long)]
    demo_tool: bool,
}

pub async fn run() -> Result<(), crate::TuiError> {
    let cli = Cli::parse();
    let config_store = ConfigStore::new(cli.config);
    let runtime_config = config_store.load(ConfigOverrides {
        provider: cli.provider,
        model: cli.model,
        base_url: cli.base_url,
        api_key_env: cli.api_key_env,
        scripted_response: cli.scripted_response,
        demo_tool: cli.demo_tool.then_some(true),
    })?;
    run_tui(TuiOptions {
        initial_prompt: cli.prompt,
        use_alt_screen: !cli.no_alt_screen,
        config_store,
        runtime_config,
    })
    .await
}
