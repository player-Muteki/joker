use thiserror::Error;

#[derive(Debug, Error)]
pub enum TuiError {
    #[error("terminal error: {0}")]
    Terminal(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("agent run failed: {0}")]
    Agent(String),
    #[error("config error: {0}")]
    Config(#[from] joker_config::ConfigError),
    #[error("event channel closed")]
    ChannelClosed,
}
