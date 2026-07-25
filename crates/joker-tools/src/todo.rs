//! Todo list tool with merge-update semantics.
//!
//! The todo list is persisted to `.joker/todos.json` in the workspace.
//! Updates merge with existing entries: setting a todo with the same `id`
//! overwrites its status while preserving fields not present in the update.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{WorkspaceTool, parse_args};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub status: String, // "pending", "in_progress", "completed"
    pub content: String,
}

/// Standalone API: merge todo items with the existing list.
///
/// Items with the same `id` overwrite existing entries. Items not mentioned
/// in the update are preserved.
pub async fn merge_todos(
    workspace: &std::path::Path,
    items: &[TodoItem],
) -> Result<Vec<TodoItem>, ToolError> {
    let todos_dir = workspace.join(".joker");
    std::fs::create_dir_all(&todos_dir)
        .map_err(|e| ToolError::Execution(format!("mkdir .joker: {e}")))?;
    let todos_path = todos_dir.join("todos.json");

    let existing: Vec<TodoItem> = if todos_path.exists() {
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

    // Merge: incoming items overwrite existing by id
    let mut map: HashMap<String, TodoItem> = existing
        .into_iter()
        .map(|item| (item.id.clone(), item))
        .collect();

    for item in items {
        map.insert(item.id.clone(), item.clone());
    }

    let merged: Vec<TodoItem> = map.into_values().collect();

    let raw = serde_json::to_string_pretty(&merged)
        .map_err(|e| ToolError::Execution(format!("serialize todos: {e}")))?;
    std::fs::write(&todos_path, &raw)
        .map_err(|e| ToolError::Execution(format!("write todos: {e}")))?;

    Ok(merged)
}

#[derive(Debug)]
pub struct TodoWriteTool {
    workspace: WorkspaceTool,
    store: Mutex<HashMap<String, TodoItem>>,
}

impl TodoWriteTool {
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
            description: "Create or update a structured task list. Items are merged by id — existing items not mentioned are preserved.".into(),
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
                    }
                },
                "required": ["todos"]
            }),
            annotations: ToolAnnotations {
                execution: ToolExecution::Sequential,
                mutating: true,
                timeout: None,
                capabilities: vec![ToolCapability::WritesFiles],
                default_approval: ApprovalRequirement::Auto,
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async move {
            let args = parse_args::<TodoArgs>(invocation.arguments)?;
            let merged = merge_todos(&self.workspace.root, &args.todos).await?;

            // Update in-memory store
            if let Ok(mut store) = self.store.lock() {
                for item in &merged {
                    store.insert(item.id.clone(), item.clone());
                }
            }

            let items_json: Vec<serde_json::Value> = merged
                .iter()
                .map(|item| json!({
                    "id": item.id,
                    "status": item.status,
                    "content": item.content,
                }))
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn merge_todos_preserves_unmentioned() {
        let tmp = std::env::temp_dir().join(format!("joker-todo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let initial = vec![TodoItem {
            id: "1".into(),
            status: "pending".into(),
            content: "First task".into(),
        }];
        merge_todos(&tmp, &initial).await.unwrap();

        let update = vec![TodoItem {
            id: "2".into(),
            status: "in_progress".into(),
            content: "Second task".into(),
        }];
        let merged = merge_todos(&tmp, &update).await.unwrap();

        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|t| t.id == "1" && t.status == "pending"));
        assert!(merged.iter().any(|t| t.id == "2" && t.status == "in_progress"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn merge_todos_overwrites_by_id() {
        let tmp = std::env::temp_dir().join(format!("joker-todo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let initial = vec![TodoItem {
            id: "1".into(),
            status: "pending".into(),
            content: "Old content".into(),
        }];
        merge_todos(&tmp, &initial).await.unwrap();

        let update = vec![TodoItem {
            id: "1".into(),
            status: "completed".into(),
            content: "Updated content".into(),
        }];
        let merged = merge_todos(&tmp, &update).await.unwrap();

        assert_eq!(merged.len(), 1);
        let item = &merged[0];
        assert_eq!(item.id, "1");
        assert_eq!(item.status, "completed");
        assert_eq!(item.content, "Updated content");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
