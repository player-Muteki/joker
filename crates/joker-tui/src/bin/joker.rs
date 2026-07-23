#[tokio::main]
async fn main() -> Result<(), joker_tui::TuiError> {
    joker_tui::cli::run().await
}
