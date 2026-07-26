use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
    #[error("invalid value: {0}")]
    InvalidValue(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse error: {0}")]
    TomlDeserialize(#[from] toml::de::Error),
    #[error("toml serialize error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}
