fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .with_target(true)
        .compact()
        .init();
}

#[tokio::main]
async fn main() -> Result<(), joker_tui::TuiError> {
    init_tracing();
    joker_tui::cli::run().await
}
