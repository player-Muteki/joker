use std::{fs, path::PathBuf, sync::Mutex};

use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput,
};
use serde_json::json;

/// A simple file-based memory store.
/// Memory entries are stored as markdown files in a `.joker-memory` directory.
pub struct MemoryStore {
    dir: PathBuf,
}

impl MemoryStore {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            dir: workspace.into().join(".joker-memory"),
        }
    }

    fn ensure_dir(&self) -> Result<(), ToolError> {
        fs::create_dir_all(&self.dir)
            .map_err(|e| ToolError::Execution(format!("cannot create memory dir: {e}")))
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("MEMORY.md")
    }
}

/// Tool for reading memory entries.
pub struct MemoryReadTool {
    store: Mutex<MemoryStore>,
}

impl MemoryReadTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            store: Mutex::new(MemoryStore::new(workspace)),
        }
    }
}

impl Tool for MemoryReadTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("memory_read"),
            description: "Read memory entries. Use 'list' to see all entries, or search with a query.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Optional search term. Omit to list all." }
                }
            }),
            annotations: ToolAnnotations {
                execution: ToolExecution::Sequential,
                mutating: false,
                timeout: None,
                capabilities: vec![ToolCapability::ReadOnly],
                default_approval: ApprovalRequirement::Auto,
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        let query = invocation
            .arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Box::pin(async move {
            let store = self.store.lock().map_err(|e| {
                ToolError::Execution(format!("memory store lock: {e}"))
            })?;
            store.ensure_dir()?;

            let index_path = store.index_path();
            if !index_path.exists() {
                return Ok(ToolOutput::new(json!({
                    "entries": [],
                    "message": "No memory entries yet. Use memory_write to create one."
                })));
            }

            let content = fs::read_to_string(&index_path)
                .map_err(|e| ToolError::Execution(format!("read memory: {e}")))?;

            let entries: Vec<&str> = if query.is_empty() {
                content.split("\n---\n").collect()
            } else {
                content
                    .split("\n---\n")
                    .filter(|entry| entry.contains(&query))
                    .collect()
            };

            Ok(ToolOutput::new(json!({
                "entries": entries,
                "count": entries.len(),
            })))
        })
    }
}

/// Tool for writing memory entries.
pub struct MemoryWriteTool {
    store: Mutex<MemoryStore>,
}

impl MemoryWriteTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            store: Mutex::new(MemoryStore::new(workspace)),
        }
    }
}

impl Tool for MemoryWriteTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("memory_write"),
            description: "Write a note to memory. The note will be persisted and readable via memory_read.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "note": { "type": "string", "description": "Memory note content." }
                },
                "required": ["note"]
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
        let note = invocation
            .arguments
            .get("note")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing 'note' field".into()))
            .unwrap_or_default()
            .to_string();

        Box::pin(async move {
            if note.is_empty() {
                return Err(ToolError::InvalidArguments("note cannot be empty".into()));
            }

            let store = self.store.lock().map_err(|e| {
                ToolError::Execution(format!("memory store lock: {e}"))
            })?;
            store.ensure_dir()?;

            let index_path = store.index_path();
            let entry = format!(
                "**Note ({ts})**\n{note}\n",
                ts = chrono_now()
            );

            let mut content = String::new();
            if index_path.exists() {
                content = fs::read_to_string(&index_path)
                    .map_err(|e| ToolError::Execution(format!("read memory: {e}")))?;
                if !content.ends_with('\n') {
                    content.push('\n');
                }
                content.push_str("\n---\n");
            }
            content.push_str(&entry);

            fs::write(&index_path, &content)
                .map_err(|e| ToolError::Execution(format!("write memory: {e}")))?;

            Ok(ToolOutput::new(json!({
                "written": true,
                "entry": entry.trim(),
            })))
        })
    }
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    let (year, month, day, hour, min) = {
        // Simple UTC date calculation
        let days = secs / 86400;
        let time_secs = secs % 86400;
        let h = time_secs / 3600;
        let m = (time_secs % 3600) / 60;

        // Year/month/day calculation (simplified)
        let mut y = 1970i64;
        let mut remaining = days as i64;
        loop {
            let days_in_year = if is_leap(y) { 366 } else { 365 };
            if remaining < days_in_year { break; }
            remaining -= days_in_year;
            y += 1;
        }
        let months_days = if is_leap(y) {
            [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        } else {
            [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        };
        let mut mo = 1u32;
        for &md in &months_days {
            if remaining < md { break; }
            remaining -= md;
            mo += 1;
        }
        (y as u32, mo, (remaining + 1) as u32, h as u32, m as u32)
    };
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02} UTC")
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use joker::ToolInvocation;
    use std::fs;

    #[tokio::test]
    async fn test_memory_write_and_read() {
        let dir = std::env::temp_dir().join(format!("joker-memory-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let write_tool = MemoryWriteTool::new(&dir);
        let read_tool = MemoryReadTool::new(&dir);

        // Write a note
        let output = write_tool
            .call(ToolInvocation {
                call_id: "1".into(),
                name: ToolName::new("memory_write"),
                arguments: json!({"note": "Important fact: Rust is awesome"}),
            })
            .await
            .unwrap();
        assert!(output.output["written"].as_bool().unwrap_or(false), "write failed");

        // Read it back
        let output = read_tool
            .call(ToolInvocation {
                call_id: "2".into(),
                name: ToolName::new("memory_read"),
                arguments: json!({}),
            })
            .await
            .unwrap();
        let entries = output.output["entries"].as_array().unwrap();
        assert!(!entries.is_empty());
        assert!(entries[0].as_str().unwrap().contains("Rust"));

        let _ = fs::remove_dir_all(&dir);
    }
}
