//! Error types for the configuration subsystem.

use thiserror::Error;

/// Errors that can occur during configuration loading, resolution, or saving.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The requested provider name does not match any built-in or custom provider.
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
    /// A configuration value failed validation.
    #[error("invalid value: {0}")]
    InvalidValue(String),
    /// An I/O error occurred while reading or writing the config file.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Failed to parse the TOML configuration file.
    #[error("toml parse error: {0}")]
    TomlDeserialize(#[from] toml::de::Error),
    /// Failed to serialize the configuration to TOML.
    #[error("toml serialize error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}
