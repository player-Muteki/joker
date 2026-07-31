//! Configuration file persistence — loading from and saving to `joker.toml`.

use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

use crate::error::ConfigError;
use crate::resolve::resolve_config;
use crate::runtime::RuntimeConfig;
use crate::types::{ConfigOverrides, FileConfig};

/// Manages reading and writing the on-disk `joker.toml` configuration file.
#[derive(Clone, Debug)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    /// Create a store with an explicit file path.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Create a store pointing at `"joker.toml"` in the current directory.
    #[must_use]
    pub fn project_default() -> Self {
        Self::new("joker.toml")
    }

    /// Returns the path to the configuration file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load and resolve the configuration, applying CLI overrides on top.
    pub fn load(&self, overrides: ConfigOverrides) -> Result<RuntimeConfig, ConfigError> {
        info!(target: "config", path = %self.path.display(), "loading configuration");
        let file = if self.path.exists() {
            let raw = fs::read_to_string(&self.path)?;
            toml::from_str::<FileConfig>(&raw)?
        } else {
            FileConfig::default()
        };
        resolve_config(file, overrides)
    }

    /// Save a [`RuntimeConfig`] to the configuration file as TOML.
    pub fn save(&self, config: &RuntimeConfig) -> Result<(), ConfigError> {
        info!(target: "config", path = %self.path.display(), "saving configuration");
        let file = config.to_file_config();
        let raw = toml::to_string_pretty(&file)?;
        fs::write(&self.path, raw)?;
        Ok(())
    }
}
