#[derive(Debug)]
pub enum UiEvent {
    Agent(joker::Event),
    RunCompleted(Result<joker::RunOutcome, String>),
    ModelDiscoveryCompleted(Result<Vec<String>, String>),
    Terminal(crossterm::event::Event),
    Tick,
}
