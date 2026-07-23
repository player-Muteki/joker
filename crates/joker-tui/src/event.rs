#[derive(Debug)]
pub enum UiEvent {
    Agent(joker::Event),
    RunCompleted(Result<joker::RunOutcome, String>),
    Terminal(crossterm::event::Event),
    Tick,
}
