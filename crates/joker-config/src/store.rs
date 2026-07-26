use std::fs;
use std::path::{Path, PathBuf};

use crate::error::ConfigError;
use crate::runtime::RuntimeConfig;
use crate::resolve::resolve_config;
use crate::types::{ConfigOverrides, FileConfig};

#[derive(Clone, Debug)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn project_default() -> Self {
        Self::new("joker.toml")
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self, overrides: ConfigOverrides) -> Result<RuntimeConfig, ConfigError> {
        let file = if self.path.exists() {
            let raw = fs::read_to_string(&self.path)?;
            toml::from_str::<FileConfig>(&raw)?
        } else {
            FileConfig::default()
        };
        resolve_config(file, overrides)
    }

    pub fn save(&self, config: &RuntimeConfig) -> Result<(), ConfigError> {
        let file = config.to_file_config();
        let raw = toml::to_string_pretty(&file)?;
        fs::write(&self.path, raw)?;
        Ok(())
    }
}
