use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Conversation, error::BoxFutureResult};

pub type SessionFuture<'a> = BoxFutureResult<'a, (), SessionError>;
pub type SessionLoadFuture<'a> = BoxFutureResult<'a, Option<SessionData>, SessionError>;
pub type SessionListFuture<'a> = BoxFutureResult<'a, Vec<SessionInfo>, SessionError>;

/// Abstraction for persisting and restoring agent sessions.
pub trait SessionStore: Send + Sync {
    /// Save a conversation snapshot.
    fn save(&self, data: SessionData) -> SessionFuture<'_>;
    /// Load the latest session data by ID.
    fn load(&self, id: &str) -> SessionLoadFuture<'_>;
    /// List all available sessions, newest first.
    fn list(&self) -> SessionListFuture<'_>;
    /// Delete a session by ID.
    fn delete(&self, id: &str) -> SessionFuture<'_>;
}

/// Metadata and conversation data for one session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionData {
    pub id: String,
    pub label: String,
    pub created_at: u64, // unix seconds
    pub updated_at: u64,
    pub model: String,
    pub conversation: Conversation,
}

/// Lightweight metadata for session listings (no full conversation).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub label: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub model: String,
    pub message_count: usize,
}

// ── JSONL Session Store ─────────────────────────────────────────────────

/// Append-only JSONL session store.
///
/// Each session is stored as `{dir}/{id}.jsonl`.
/// Each line is a JSON object representing a single message.
/// The first line contains the session metadata header.
pub struct JsonlSessionStore {
    dir: PathBuf,
}

impl JsonlSessionStore {
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self, SessionError> {
        let dir = dir.into();
        fs::create_dir_all(&dir).map_err(|e| SessionError::Io(e.to_string()))?;
        Ok(Self { dir })
    }

    pub fn path_for(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.jsonl"))
    }

    fn meta_path(&self) -> PathBuf {
        self.dir.join("index.json")
    }

    fn load_index(&self) -> Vec<SessionInfo> {
        let path = self.meta_path();
        if path.exists() {
            fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    fn save_index(&self, sessions: &[SessionInfo]) -> Result<(), SessionError> {
        let json = serde_json::to_string_pretty(sessions)
            .map_err(|e| SessionError::Serde(e.to_string()))?;
        fs::write(self.meta_path(), json)
            .map_err(|e| SessionError::Io(e.to_string()))?;
        Ok(())
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

impl SessionStore for JsonlSessionStore {
    fn save(&self, data: SessionData) -> SessionFuture<'_> {
        let path = self.path_for(&data.id);
        Box::pin(async move {
            let mut lines = Vec::new();
            // Header line: metadata (prefixed with #)
            let header = serde_json::json!({
                "#": "session",
                "id": data.id,
                "label": data.label,
                "created_at": data.created_at,
                "model": data.model,
            });
            lines.push(serde_json::to_string(&header).map_err(|e| SessionError::Serde(e.to_string()))?);

            // Message lines
            for msg in data.conversation.messages() {
                let line = serde_json::to_string(msg).map_err(|e| SessionError::Serde(e.to_string()))?;
                lines.push(line);
            }

            let content = lines.join("\n");
            fs::write(&path, &content).map_err(|e| SessionError::Io(e.to_string()))?;

            // Update index
            let mut index = self.load_index();
            if let Some(existing) = index.iter_mut().find(|s| s.id == data.id) {
                existing.updated_at = Self::now();
                existing.message_count = data.conversation.messages().len();
            } else {
                index.push(SessionInfo {
                    id: data.id.clone(),
                    label: data.label,
                    created_at: data.created_at,
                    updated_at: Self::now(),
                    model: data.model,
                    message_count: data.conversation.messages().len(),
                });
            }
            self.save_index(&index)?;

            Ok(())
        })
    }

    fn load(&self, id: &str) -> SessionLoadFuture<'_> {
        let id = id.to_string();
        let path = self.path_for(&id);
        let index = self.load_index();
        let id_copy = id.clone();
        let meta = index.into_iter().find(|s| s.id == id);

        Box::pin(async move {
            if !path.exists() {
                return Ok(None);
            }
            let content =
                fs::read_to_string(&path).map_err(|e| SessionError::Io(e.to_string()))?;
            let mut lines: Vec<&str> = content.lines().collect();
            if lines.is_empty() {
                return Ok(None);
            }

            // Parse header
            let header_line = lines.remove(0);
            let header: serde_json::Value =
                serde_json::from_str(header_line).map_err(|e| SessionError::Serde(e.to_string()))?;

            let _sid = header["id"].as_str().unwrap_or(&id_copy).to_string();
            let label = header["label"].as_str().unwrap_or("").to_string();
            let created_at = header["created_at"].as_u64().unwrap_or(0);
            let model = header["model"].as_str().unwrap_or("").to_string();

            // Parse messages
            let mut conversation = Conversation::new();
            for line in lines {
                if let Ok(msg) = serde_json::from_str::<crate::Message>(line) {
                    conversation.push(msg);
                }
            }

            let meta = meta.unwrap_or(SessionInfo {
                id: id.clone(),
                label: label.clone(),
                created_at,
                updated_at: created_at,
                model: model.clone(),
                message_count: conversation.messages().len(),
            });

            Ok(Some(SessionData {
                id,
                label,
                created_at,
                updated_at: meta.updated_at,
                model,
                conversation,
            }))
        })
    }

    fn list(&self) -> SessionListFuture<'_> {
        let index = self.load_index();
        Box::pin(async move {
            let mut sessions = index;
            sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            Ok(sessions)
        })
    }

    fn delete(&self, id: &str) -> SessionFuture<'_> {
        let id = id.to_string();
        let path = self.path_for(&id);
        Box::pin(async move {
            let _ = fs::remove_file(&path);
            let mut index = self.load_index();
            index.retain(|s| s.id != id);
            self.save_index(&index)?;
            Ok(())
        })
    }
}

// ── Errors ──────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("io error: {0}")]
    Io(String),
    #[error("serialization error: {0}")]
    Serde(String),
    #[error("session not found: {0}")]
    NotFound(String),
}

#[cfg(test)]

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Conversation, Message};

    struct TempDir {
        path: PathBuf,
        _name: String,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let id = std::process::id();
            let path = std::env::temp_dir().join(format!("joker-session-{id}-{name}"));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self { path, _name: name.to_string() }
        }
        fn path(&self) -> PathBuf { self.path.clone() }
    }

    impl Drop for TempDir {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.path); }
    }

    fn test_store(name: &str) -> (TempDir, JsonlSessionStore) {
        let dir = TempDir::new(name);
        let store = JsonlSessionStore::new(dir.path()).unwrap();
        (dir, store)
    }

    #[tokio::test]
    async fn test_save_and_load() {
        let (_dir, store) = test_store("sl");
        let mut conv = Conversation::new();
        conv.push(Message::user("hello"));
        conv.push(Message::assistant(vec![crate::Content::text("world")]));
        let data = SessionData {
            id: "s1".into(), label: "Test".into(),
            created_at: 1000, updated_at: 1000,
            model: "m".into(), conversation: conv,
        };
        store.save(data).await.unwrap();
        let loaded = store.load("s1").await.unwrap().unwrap();
        assert_eq!(loaded.id, "s1");
        assert_eq!(loaded.conversation.messages().len(), 2);
    }

    #[tokio::test]
    async fn test_list() {
        let (_dir, store) = test_store("list");
        let mut conv = Conversation::new();
        conv.push(Message::user("test"));
        let data = SessionData {
            id: "l1".into(), label: "L".into(),
            created_at: 1000, updated_at: 1000,
            model: "m".into(), conversation: conv,
        };
        store.save(data).await.unwrap();
        let sessions = store.list().await.unwrap();
        assert!(sessions.iter().any(|s| s.id == "l1"));
    }

    #[tokio::test]
    async fn test_delete() {
        let (_dir, store) = test_store("del");
        let mut conv = Conversation::new();
        conv.push(Message::user("delete me"));
        let data = SessionData {
            id: "d1".into(), label: "D".into(),
            created_at: 1000, updated_at: 1000,
            model: "m".into(), conversation: conv,
        };
        store.save(data).await.unwrap();
        store.delete("d1").await.unwrap();
        assert!(store.load("d1").await.unwrap().is_none());
    }
}
