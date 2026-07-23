use clap::Parser;

use joker_tui::{TuiOptions, run_tui};

#[derive(Debug, Parser)]
#[command(name = "joker-tui", about = "Terminal UI for Joker agent kernel")]
struct Cli {
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long)]
    no_alt_screen: bool,
    #[arg(long, default_value = "Hello from Joker TUI.")]
    scripted_response: String,
    #[arg(long)]
    demo_tool: bool,
}

#[tokio::main]
async fn main() -> Result<(), joker_tui::TuiError> {
    let cli = Cli::parse();
    run_tui(TuiOptions {
        initial_prompt: cli.prompt,
        use_alt_screen: !cli.no_alt_screen,
        scripted_response: cli.scripted_response,
        demo_tool: cli.demo_tool,
    })
    .await
}
