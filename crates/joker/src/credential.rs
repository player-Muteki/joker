use std::{
    collections::HashMap,
    fs,
    path::{PathBuf},
};

use thiserror::Error;
use tracing::{debug, info};

/// Errors from credential operations.
#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("io error: {0}")]
    /// An I/O error occurred (file read/write).
    Io(String),
    #[error("serialization error: {0}")]
    /// A serialization/deserialization error occurred (JSON).
    Serde(String),
}

/// In-memory credential store with optional file persistence (auth.json).
///
/// Mirrors pi's AuthStorage pattern: credentials are stored in memory
/// at runtime and can be persisted to a JSON file on save().
/// Each provider stores exactly one API key credential.
///
/// ```
/// use joker::CredentialStore;
///
/// let mut store = CredentialStore::new();
/// store.set("deepseek", "sk-xxx".to_string());
/// assert!(store.has("deepseek"));
/// ```
#[derive(Clone, Debug, Default)]
pub struct CredentialStore {
    credentials: HashMap<String, String>,
    path: Option<PathBuf>,
    dirty: bool,
}

impl CredentialStore {
    /// Create an in-memory-only credential store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a credential store backed by a JSON file.
    /// Loads existing credentials from the file if it exists.
    #[must_use]
    pub fn with_file(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut store = Self {
            credentials: HashMap::new(),
            path: Some(path.clone()),
            dirty: false,
        };
        if path.exists()
            && let Ok(data) = fs::read_to_string(&path)
                && let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&data) {
                    store.credentials = map;
                }
        store
    }

    /// Get the API key for a provider.
    /// Checks credential store first, then falls back to env var.
    /// This mirrors pi's `getApiKey()` priority: store > env.
    #[must_use]
    pub fn get(&self, provider_id: &str) -> Option<String> {
        debug!(target: "credential", provider = %provider_id, "getting API key");
        // Try stored credential first
        if let Some(key) = self.credentials.get(provider_id) {
            return Some(key.clone());
        }
        // Fall back to environment variable
        let env_var = format!("{}_API_KEY", provider_id.to_uppercase());
        std::env::var(env_var).ok()
    }

    /// Set the API key for a provider (in-memory).
    /// Call `save()` to persist to disk.
    pub fn set(&mut self, provider_id: &str, api_key: String) {
        info!(target: "credential", provider = %provider_id, "setting API key");
        self.credentials.insert(provider_id.to_string(), api_key);
        self.dirty = true;
    }

    /// Remove a credential.
    pub fn remove(&mut self, provider_id: &str) {
        info!(target: "credential", provider = %provider_id, "deleting API key");
        self.credentials.remove(provider_id);
        self.dirty = true;
    }

    /// Check whether a credential exists (in memory or env var).
    /// Env var check is a convenience so provider switching detects
    /// pre-configured env vars without an explicit store entry.
    #[must_use]
    pub fn has(&self, provider_id: &str) -> bool {
        if self.credentials.contains_key(provider_id) {
            return true;
        }
        let env_var = format!("{}_API_KEY", provider_id.to_uppercase());
        std::env::var(env_var).is_ok()
    }

    /// List all provider IDs that have credentials stored in memory.
    #[must_use]
    pub fn list(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self.credentials.keys().map(String::as_str).collect();
        keys.sort();
        keys
    }

    /// Persist credentials to the JSON file.
    ///
    /// Writes with strict `0o600` permissions (owner read/write only)
    /// to protect API key secrets. This mirrors pi's `auth.json` pattern.
    pub fn save(&self) -> Result<(), CredentialError> {
        if let Some(path) = &self.path {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| CredentialError::Io(e.to_string()))?;
            }
            let json = serde_json::to_string_pretty(&self.credentials)
                .map_err(|e| CredentialError::Serde(e.to_string()))?;
            fs::write(path, json)
                .map_err(|e| CredentialError::Io(e.to_string()))?;
            // Set 0o600 permissions on Unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(metadata) = fs::metadata(path) {
                    let mut perms = metadata.permissions();
                    perms.set_mode(0o600);
                    let _ = fs::set_permissions(path, perms);
                }
            }
        }
        Ok(())
    }

    /// Number of stored credentials.
    #[must_use]
    pub fn len(&self) -> usize {
        self.credentials.len()
    }

    /// Returns `true` if the credential store contains no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.credentials.is_empty()
    }
}
