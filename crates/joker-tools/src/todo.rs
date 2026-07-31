//! Todo list tool with merge-update semantics and progress gating.
//!
//! The todo list is persisted to `.joker/todos.json` in the workspace.
//! Updates merge with existing entries by default: setting a todo with the
//! same `id` overwrites its status while preserving fields not present in
//! the update.
//!
//! # Progress Gating
//!
//! Status transitions are enforced to prevent regressions:
//! - `pending` → `in_progress` (allowed)
//! - `pending` → `completed` (allowed)
//! - `in_progress` → `completed` (allowed)
//! - `in_progress` → `pending` (rejected)
//! - `completed` → `pending` or `in_progress` (rejected)
//!
//! Use `overwrite: true` to replace the entire list (bypasses gating).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::workspace::{WorkspaceTool, parse_args};

/// A single todo item with status tracking and progress-gated transitions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TodoItem {
    /// Unique identifier for this todo item.
    pub id: String,
    /// Current status: `"pending"`, `"in_progress"`, or `"completed"`.
    pub status: String,
    /// Description of the todo item.
    pub content: String,
}

/// Valid status transitions (current → next).
const ALLOWED_TRANSITIONS: &[(&str, &[&str])] = &[
    ("pending", &["pending", "in_progress", "completed"]),
    ("in_progress", &["in_progress", "completed"]),
    ("completed", &["completed"]),
];

fn is_valid_transition(current: &str, next: &str) -> bool {
    ALLOWED_TRANSITIONS
        .iter()
        .find(|(c, _)| *c == current)
        .map(|(_, allowed)| allowed.contains(&next))
        .unwrap_or(true) // unknown status → allow
}

/// Standalone API: merge todo items with the existing list.
///
/// Items with the same `id` overwrite existing entries. Items not mentioned
/// in the update are preserved. Status transitions are gated unless
/// `overwrite` is true.
pub async fn merge_todos(
    workspace: &std::path::Path,
    items: &[TodoItem],
    overwrite: bool,
) -> Result<Vec<TodoItem>, ToolError> {
    let todos_dir = workspace.join(".joker");
    std::fs::create_dir_all(&todos_dir)
        .map_err(|e| ToolError::Execution(format!("mkdir .joker: {e}")))?;
    let todos_path = todos_dir.join("todos.json");

    let mut existing: Vec<TodoItem> = if todos_path.exists() && !overwrite {
        let raw = std::fs::read_to_string(&todos_path)
            .map_err(|e| ToolError::Execution(format!("read todos: {e}")))?;
        if raw.trim().is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(&raw)
                .map_err(|e| ToolError::Execution(format!("parse todos: {e}")))?
        }
    } else {
        Vec::new()
    };

    // Build existing map for gating
    let existing_map: HashMap<String, TodoItem> = existing
        .drain(..)
        .map(|item| (item.id.clone(), item))
        .collect();

    // Merge: incoming items overwrite existing by id
    let mut map: HashMap<String, TodoItem> = existing_map;

    for item in items {
        if !overwrite {
            // Check progress gating
            if let Some(current) = map.get(&item.id)
                && !is_valid_transition(&current.status, &item.status)
            {
                return Err(ToolError::InvalidArguments(format!(
                    "invalid status transition for '{}': '{}' → '{}'. \
                     Allowed: pending→in_progress, pending→completed, in_progress→completed. \
                     Use overwrite=true to bypass.",
                    item.id, current.status, item.status,
                )));
            }
        }
        map.insert(item.id.clone(), item.clone());
    }

    let merged: Vec<TodoItem> = map.into_values().collect();

    let raw = serde_json::to_string_pretty(&merged)
        .map_err(|e| ToolError::Execution(format!("serialize todos: {e}")))?;
    std::fs::write(&todos_path, &raw)
        .map_err(|e| ToolError::Execution(format!("write todos: {e}")))?;

    Ok(merged)
}

/// A tool that creates and updates todo items with progress gating.
#[derive(Debug)]
pub struct TodoWriteTool {
    workspace: WorkspaceTool,
    store: Mutex<HashMap<String, TodoItem>>,
}

impl TodoWriteTool {
    /// Create a new `TodoWriteTool` rooted at the given workspace path.
    pub fn new(root: PathBuf) -> Self {
        Self {
            workspace: WorkspaceTool::new(root),
            store: Mutex::new(HashMap::new()),
        }
    }
}

impl Tool for TodoWriteTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("todo_write"),
            description: "Create or update a structured task list. Items are merged by id — existing items not mentioned are preserved. Status transitions are gated: pending→in_progress, pending→completed, in_progress→completed. Use overwrite=true to replace the entire list.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "Todo items to merge into the list.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {"type": "string", "description": "Unique identifier for this todo."},
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"],
                                    "description": "Current status of the todo."
                                },
                                "content": {"type": "string", "description": "Description of the todo."}
                            },
                            "required": ["id", "status", "content"]
                        }
                    },
                    "overwrite": {
                        "type": "boolean",
                        "description": "When true, replace the entire list instead of merging. Bypasses progress gating. Default: false."
                    }
                },
                "required": ["todos"]
            }),
            annotations: ToolAnnotations::from_capabilities(
                ToolExecution::Sequential,
                vec![ToolCapability::WritesFiles],
                None,
                ApprovalRequirement::Auto,
            ),
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async move {
            let args = parse_args::<TodoArgs>(invocation.arguments)?;
            let overwrite = args.overwrite.unwrap_or(false);
            let merged = merge_todos(&self.workspace.root, &args.todos, overwrite).await?;

            // Update in-memory store
            if let Ok(mut store) = self.store.lock() {
                for item in &merged {
                    store.insert(item.id.clone(), item.clone());
                }
            }

            let items_json: Vec<serde_json::Value> = merged
                .iter()
                .map(|item| {
                    json!({
                        "id": item.id,
                        "status": item.status,
                        "content": item.content,
                    })
                })
                .collect();

            Ok(ToolOutput::new(json!({
                "count": items_json.len(),
                "todos": items_json,
            })))
        })
    }
}

#[derive(Debug, Deserialize)]
struct TodoArgs {
    todos: Vec<TodoItem>,
    overwrite: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn merge_todos_preserves_unmentioned() {
        let tmp = std::env::temp_dir().join(format!("joker-todo-preserve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let initial = vec![TodoItem {
            id: "1".into(),
            status: "pending".into(),
            content: "First task".into(),
        }];
        merge_todos(&tmp, &initial, false).await.unwrap();

        let update = vec![TodoItem {
            id: "2".into(),
            status: "in_progress".into(),
            content: "Second task".into(),
        }];
        let merged = merge_todos(&tmp, &update, false).await.unwrap();

        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|t| t.id == "1" && t.status == "pending"));
        assert!(
            merged
                .iter()
                .any(|t| t.id == "2" && t.status == "in_progress")
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn merge_todos_overwrites_by_id() {
        let tmp = std::env::temp_dir().join(format!("joker-todo-overwrite-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let initial = vec![TodoItem {
            id: "1".into(),
            status: "pending".into(),
            content: "Old content".into(),
        }];
        merge_todos(&tmp, &initial, false).await.unwrap();

        let update = vec![TodoItem {
            id: "1".into(),
            status: "completed".into(),
            content: "Updated content".into(),
        }];
        let merged = merge_todos(&tmp, &update, false).await.unwrap();

        assert_eq!(merged.len(), 1);
        let item = &merged[0];
        assert_eq!(item.id, "1");
        assert_eq!(item.status, "completed");
        assert_eq!(item.content, "Updated content");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn rejects_invalid_progress() {
        let tmp = std::env::temp_dir().join(format!("joker-todo-gate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let initial = vec![TodoItem {
            id: "1".into(),
            status: "completed".into(),
            content: "Done task".into(),
        }];
        merge_todos(&tmp, &initial, false).await.unwrap();

        // Try to go back from completed → pending
        let update = vec![TodoItem {
            id: "1".into(),
            status: "pending".into(),
            content: "Done task".into(),
        }];
        let result = merge_todos(&tmp, &update, false).await;
        assert!(
            result.is_err(),
            "should reject completed→pending transition"
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid status transition")
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn overwrite_bypasses_gating() {
        let tmp =
            std::env::temp_dir().join(format!("joker-todo-overwrite2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let initial = vec![TodoItem {
            id: "1".into(),
            status: "completed".into(),
            content: "Done".into(),
        }];
        merge_todos(&tmp, &initial, false).await.unwrap();

        // Overwrite with a fresh list
        let fresh = vec![TodoItem {
            id: "1".into(),
            status: "pending".into(),
            content: "Redo".into(),
        }];
        let merged = merge_todos(&tmp, &fresh, true).await.unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].status, "pending");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
