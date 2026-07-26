//! Session persistence and management for agent conversations.
//!
//! The [`SessionStore`] trait defines the interface for saving, loading,
//! listing, deleting, and forking sessions. [`JsonlSessionStore`] is an
//! append-only JSONL-backed implementation that stores each session as
//! `{id}.jsonl` with an `index.json` for fast metadata lookups.

use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, error, info};

use crate::{Conversation, error::BoxFutureResult};

/// Future type returned by [`SessionStore::save`] and [`SessionStore::delete`].
pub type SessionFuture<'a> = BoxFutureResult<'a, (), SessionError>;
/// Future type returned by [`SessionStore::load`] and [`SessionStore::fork`].
pub type SessionLoadFuture<'a> = BoxFutureResult<'a, Option<SessionData>, SessionError>;
/// Future type returned by [`SessionStore::list`], [`SessionStore::path_to_root`], and [`SessionStore::children`].
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

    // ── Tree / Fork ─────────────────────────────────────────────────────

    /// Fork: create a new session that shares the parent's message history
    /// up to the fork point. Returns the new child session's data.
    fn fork(&self, parent_id: &str, label: String, agent_name: String, model: String) -> SessionLoadFuture<'_>;

    /// Load the full tree path from a leaf session back to the root.
    /// Returns sessions ordered root → leaf.
    fn path_to_root(&self, leaf_id: &str) -> SessionListFuture<'_>;

    /// List all child sessions (direct forks) of a given session.
    fn children(&self, session_id: &str) -> SessionListFuture<'_>;
}

/// Metadata and conversation data for one session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionData {
    /// Unique identifier for the session.
    pub id: String,
    /// Human-readable display label for the session.
    pub label: String,
    /// Unix timestamp (seconds) when the session was created.
    pub created_at: u64,
    /// Unix timestamp (seconds) of the most recent update.
    pub updated_at: u64,
    /// Model identifier used for this session.
    pub model: String,
    /// Name of the agent associated with this session.
    pub agent_name: String,
    /// Parent session ID for tree/fork branching (`None` = root session).
    pub parent_id: Option<String>,
    /// Root session ID for tree navigation (same as `id` for root sessions).
    pub root_id: String,
    /// Full [`Conversation`] history for this session.
    pub conversation: Conversation,
}

/// Lightweight metadata for session listings (no full conversation).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Unique identifier for the session.
    pub id: String,
    /// Human-readable display label for the session.
    pub label: String,
    /// Unix timestamp (seconds) when the session was created.
    pub created_at: u64,
    /// Unix timestamp (seconds) of the most recent update.
    pub updated_at: u64,
    /// Model identifier used for this session.
    pub model: String,
    /// Name of the agent associated with this session.
    pub agent_name: String,
    /// Parent session ID for tree/fork branching.
    pub parent_id: Option<String>,
    /// Root session ID.
    pub root_id: String,
    /// Number of messages in the session conversation.
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
    /// Create a new [`JsonlSessionStore`] rooted at `dir`.
    ///
    /// Creates the directory if it does not already exist.
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self, SessionError> {
        let dir = dir.into();
        fs::create_dir_all(&dir).map_err(|e| SessionError::Io(e.to_string()))?;
        Ok(Self { dir })
    }

    /// Return the filesystem path for the JSONL file of a given session ID.
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
        let session_id = data.id.clone();
        let msg_count = data.conversation.messages().len();
        Box::pin(async move {
            info!(target: "session", session_id = %session_id, message_count = msg_count, "saving session");
            let mut lines = Vec::new();
            let now = Self::now();
            let header = serde_json::json!({
                "#": "session",
                "id": session_id,
                "label": data.label,
                "created_at": data.created_at,
                "updated_at": now,
                "model": data.model,
                "agent_name": data.agent_name,
                "parent_id": data.parent_id,
                "root_id": data.root_id,
            });
            lines.push(serde_json::to_string(&header).map_err(|e| {
                error!(target: "session", session_id = %session_id, error = %e, "failed to serialize session header");
                SessionError::Serde(e.to_string())
            })?);

            for msg in data.conversation.messages() {
                let line = serde_json::to_string(msg).map_err(|e| SessionError::Serde(e.to_string()))?;
                lines.push(line);
            }

            let content = lines.join("\n");
            fs::write(&path, &content).map_err(|e| {
                error!(target: "session", session_id = %session_id, error = %e, "failed to write session file");
                SessionError::Io(e.to_string())
            })?;

            let mut index = self.load_index();
            if let Some(existing) = index.iter_mut().find(|s| s.id == session_id) {
                existing.updated_at = now;
                existing.message_count = data.conversation.messages().len();
            } else {
                index.push(SessionInfo {
                    id: session_id.clone(),
                    label: data.label,
                    created_at: data.created_at,
                    updated_at: now,
                    model: data.model,
                    agent_name: data.agent_name,
                    parent_id: data.parent_id,
                    root_id: data.root_id,
                    message_count: data.conversation.messages().len(),
                });
            }
            self.save_index(&index).map_err(|e| {
                error!(target: "session", session_id = %session_id, error = %e, "failed to save session index");
                e
            })?;

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
            info!(target: "session", session_id = %id_copy, "loading session");
            if !path.exists() {
                return Ok(None);
            }
            let content =
                fs::read_to_string(&path).map_err(|e| {
                    error!(target: "session", session_id = %id_copy, error = %e, "failed to read session file");
                    SessionError::Io(e.to_string())
                })?;
            let mut lines: Vec<&str> = content.lines().collect();
            if lines.is_empty() {
                return Ok(None);
            }

            let header_line = lines.remove(0);
            let header: serde_json::Value =
                serde_json::from_str(header_line).map_err(|e| {
                    error!(target: "session", session_id = %id_copy, error = %e, "failed to parse session header");
                    SessionError::Serde(e.to_string())
                })?;

            let label = header["label"].as_str().unwrap_or("").to_string();
            let created_at = header["created_at"].as_u64().unwrap_or(0);
            let model = header["model"].as_str().unwrap_or("").to_string();
            let agent_name = header.get("agent_name").and_then(|v| v.as_str()).unwrap_or("default").to_string();
            let parent_id = header.get("parent_id").and_then(|v| v.as_str()).map(String::from);
            let root_id = header.get("root_id").and_then(|v| v.as_str()).unwrap_or(&id_copy).to_string();

            let mut conversation = Conversation::new();
            for line in lines {
                if let Ok(msg) = serde_json::from_str::<crate::Message>(line) {
                    conversation.push(msg);
                }
            }

            let meta = meta.unwrap_or(SessionInfo {
                id: id_copy.clone(),
                label: label.clone(),
                created_at,
                updated_at: created_at,
                model: model.clone(),
                agent_name: agent_name.clone(),
                parent_id: parent_id.clone(),
                root_id: root_id.clone(),
                message_count: conversation.messages().len(),
            });

            info!(target: "session", session_id = %id_copy, message_count = conversation.messages().len(), "session loaded");
            Ok(Some(SessionData {
                id: id_copy,
                label,
                created_at,
                updated_at: meta.updated_at,
                model,
                agent_name,
                parent_id,
                root_id,
                conversation,
            }))
        })
    }

    fn list(&self) -> SessionListFuture<'_> {
        let index = self.load_index();
        Box::pin(async move {
            debug!(target: "session", count = index.len(), "listing sessions");
            let mut sessions = index;
            sessions.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
            Ok(sessions)
        })
    }

    fn delete(&self, id: &str) -> SessionFuture<'_> {
        let id = id.to_string();
        let path = self.path_for(&id);
        Box::pin(async move {
            info!(target: "session", session_id = %id, "deleting session");
            let _ = fs::remove_file(&path);
            let mut index = self.load_index();
            index.retain(|s| s.id != id);
            let children: Vec<String> = index.iter()
                .filter(|s| s.parent_id.as_deref() == Some(&id))
                .map(|s| s.id.clone())
                .collect();
            for child_id in children {
                let _ = fs::remove_file(self.path_for(&child_id));
            }
            index.retain(|s| s.parent_id.as_deref() != Some(&id));
            self.save_index(&index).map_err(|e| {
                error!(target: "session", session_id = %id, error = %e, "failed to save index after deletion");
                e
            })?;
            Ok(())
        })
    }

    // ── Tree / Fork ─────────────────────────────────────────────────────

    fn fork(&self, parent_id: &str, label: String, agent_name: String, model: String) -> SessionLoadFuture<'_> {
        let parent_id = parent_id.to_string();
        let id = format!("fork-{}-{}", parent_id, Self::now());
        info!(target: "session", parent_id = %parent_id, child_id = %id, "forking session");
        Box::pin(async move {
            let parent = self.load(&parent_id).await?
                .ok_or_else(|| {
                    error!(target: "session", parent_id = %parent_id, "parent session not found for fork");
                    SessionError::NotFound(parent_id.clone())
                })?;

            // Determine root_id: use parent's root_id or parent's own id
            let root_id = if parent.root_id.is_empty() { parent_id.clone() } else { parent.root_id.clone() };

            let child = SessionData {
                id,
                label,
                created_at: Self::now(),
                updated_at: Self::now(),
                model,
                agent_name,
                parent_id: Some(parent_id),
                root_id,
                conversation: parent.conversation,
            };

            // Save the fork
            self.save(child.clone()).await?;
            Ok(Some(child))
        })
    }

    fn path_to_root(&self, leaf_id: &str) -> SessionListFuture<'_> {
        let leaf_id = leaf_id.to_string();
        let index = self.load_index();
        Box::pin(async move {
            let mut path: Vec<SessionInfo> = Vec::new();
            let mut current_id: Option<String> = Some(leaf_id);
            while let Some(id) = current_id.take() {
                if let Some(info) = index.iter().find(|s| s.id == id).cloned() {
                    current_id = info.parent_id.clone();
                    path.push(info);
                } else {
                    break;
                }
            }
            path.reverse(); // root → leaf order
            Ok(path)
        })
    }

    fn children(&self, session_id: &str) -> SessionListFuture<'_> {
        let session_id = session_id.to_string();
        let index = self.load_index();
        Box::pin(async move {
            let children: Vec<SessionInfo> = index.into_iter()
                .filter(|s| s.parent_id.as_deref() == Some(&session_id))
                .collect();
            Ok(children)
        })
    }
}

// ── Errors ──────────────────────────────────────────────────────────────

/// Errors that can occur during session store operations.
#[derive(Debug, Error)]
pub enum SessionError {
    /// An I/O error occurred (e.g. file read/write failure).
    #[error("io error: {0}")]
    Io(String),
    /// A serialization or deserialization error occurred.
    #[error("serialization error: {0}")]
    Serde(String),
    /// The requested session was not found.
    #[error("session not found: {0}")]
    NotFound(String),
}

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
            model: "m".into(), agent_name: "build".into(),
            parent_id: None, root_id: "s1".into(),
            conversation: conv,
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
            model: "m".into(), agent_name: "build".into(),
            parent_id: None, root_id: "l1".into(),
            conversation: conv,
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
            model: "m".into(), agent_name: "build".into(),
            parent_id: None, root_id: "d1".into(),
            conversation: conv,
        };
        store.save(data).await.unwrap();
        store.delete("d1").await.unwrap();
        assert!(store.load("d1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_fork_and_path_to_root() {
        let (_dir, store) = test_store("fork");
        let mut conv = Conversation::new();
        conv.push(Message::user("root message"));
        let root_data = SessionData {
            id: "root1".into(), label: "Root".into(),
            created_at: 1000, updated_at: 1000,
            model: "m".into(), agent_name: "build".into(),
            parent_id: None, root_id: "root1".into(),
            conversation: conv,
        };
        store.save(root_data).await.unwrap();

        // Fork
        let forked = store.fork("root1", "Fork 1".into(), "plan".into(), "m".into()).await.unwrap().unwrap();
        assert_eq!(forked.parent_id, Some("root1".into()));
        assert_eq!(forked.root_id, "root1");
        assert_eq!(forked.conversation.messages().len(), 1);

        // Path to root: should return [root, fork]
        let path = store.path_to_root(&forked.id).await.unwrap();
        assert_eq!(path.len(), 2);
        assert_eq!(path[0].id, "root1");
        assert_eq!(path[1].id, forked.id);

        // Children
        let children = store.children("root1").await.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, forked.id);
    }
}
