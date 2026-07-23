use thiserror::Error;

#[derive(Debug, Error)]
pub enum TuiError {
    #[error("terminal error: {0}")]
    Terminal(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("agent run failed: {0}")]
    Agent(String),
    #[error("event channel closed")]
    ChannelClosed,
}
