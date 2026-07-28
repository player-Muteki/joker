//! File-based memory store with YAML frontmatter, @path references, and quick append.
//!
//! Memory entries are stored as markdown files in a `.joker-memory` directory.
//!
//! # YAML Frontmatter
//!
//! Entries can begin with YAML frontmatter delimited by `---`:
//! ```markdown
//! ---
//! type: project
//! tags: [rust, async]
//! ---
//! Note content here
//! ```
//!
//! # @path References
//!
//! When writing a note, `@path/to/file` (relative to workspace root) is replaced
//! with the content of that file. Use an extended form `@path/to/file#L10-L20` to
//! reference specific lines.
//!
//! # Quick Append
//!
//! If the note starts with `#`, it is appended as a single line without timestamp formatting.

use std::{fs, path::PathBuf, sync::Mutex};

use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput,
};
use serde_json::json;

/// A simple file-based memory store.
pub struct MemoryStore {
    dir: PathBuf,
    workspace: PathBuf,
}

impl MemoryStore {
    /// Create a new `MemoryStore` rooted at `.joker-memory` under the workspace.
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let workspace = workspace.into();
        Self {
            dir: workspace.join(".joker-memory"),
            workspace,
        }
    }

    fn ensure_dir(&self) -> Result<(), ToolError> {
        fs::create_dir_all(&self.dir)
            .map_err(|e| ToolError::Execution(format!("cannot create memory dir: {e}")))
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("MEMORY.md")
    }

    /// Expand @path references in a note string.
    /// Replaces `@relative/path` with the file content, optionally with line ranges.
    /// Matches `@` followed by a non-whitespace path string.
    fn expand_references(&self, note: &str) -> String {
        let mut result = String::with_capacity(note.len());
        let bytes = note.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        while i < len {
            if bytes[i] == b'@' {
                // Find end of the reference (whitespace or end)
                let start = i + 1;
                let mut end = start;
                while end < len && !bytes[end].is_ascii_whitespace() {
                    end += 1;
                }
                let reference = &note[start..end];
                if !reference.is_empty() {
                    let (path_str, line_range) = if let Some(pos) = reference.find('#') {
                        (&reference[..pos], Some(&reference[pos + 1..]))
                    } else {
                        (reference, None)
                    };

                    let target = self.workspace.join(path_str);
                    if target.exists() && target.starts_with(&self.workspace)
                        && let Ok(content) = fs::read_to_string(&target)
                    {
                        let snippet = if let Some(range) = line_range {
                            extract_lines(&content, range)
                        } else {
                            content
                        };
                        result.push_str(&snippet);
                        i = end;
                        continue;
                    }
                }
                // If reference wasn't resolved, keep the @ as-is
                result.push('@');
                i += 1;
            } else {
                result.push(note[i..].chars().next().unwrap_or(' '));
                i += note[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            }
        }
        result
    }
}

/// Extract a line range like "L10" or "L10-L20" from content.
fn extract_lines(content: &str, range: &str) -> String {
    let range = range.strip_prefix('L').unwrap_or(range);
    let (start, end) = if let Some((s, e)) = range.split_once("-L") {
        (s.parse::<usize>().unwrap_or(1), e.parse::<usize>().unwrap_or(usize::MAX))
    } else if let Some((s, e)) = range.split_once('-') {
        (s.parse::<usize>().unwrap_or(1), e.parse::<usize>().unwrap_or(usize::MAX))
    } else {
        (range.parse::<usize>().unwrap_or(1), usize::MAX)
    };

    let start = start.saturating_sub(1);
    let lines: Vec<&str> = content.lines().collect();
    let end = end.min(lines.len());
    if start >= lines.len() {
        return String::new();
    }
    lines[start..end].join("\n")
}

/// Parse YAML frontmatter from content. Returns (metadata_json, body).
/// Frontmatter is delimited by `---\n...\n---` at the start.
fn parse_frontmatter(content: &str) -> (serde_json::Value, &str) {
    let content = content.trim_start();
    if !content.starts_with("---\n") {
        return (json!({}), content);
    }

    let rest = &content[4..]; // skip "---\n"
    if let Some(end) = rest.find("\n---") {
        let fm_str = &rest[..end];
        let body = rest[end + 4..].trim_start();
        let mut meta = serde_json::Map::new();
        for line in fm_str.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let k = key.trim().to_string();
                let v = value.trim().to_string();
                meta.insert(k, json!(v));
            }
        }
        (serde_json::Value::Object(meta), body)
    } else {
        (json!({}), content)
    }
}

/// Tool for reading memory entries.
pub struct MemoryReadTool {
    store: Mutex<MemoryStore>,
}

impl MemoryReadTool {
    /// Create a new `MemoryReadTool` for the given workspace.
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
            annotations: ToolAnnotations::from_capabilities(
                ToolExecution::Sequential,
                vec![ToolCapability::ReadOnly],
                None,
                ApprovalRequirement::Auto,
            ),
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

            let raw_entries: Vec<&str> = content.split("\n===\n").collect();
            let mut entries: Vec<serde_json::Value> = Vec::new();

            for raw in raw_entries {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let (meta, body) = parse_frontmatter(trimmed);
                let body_text = body.trim();
                if query.is_empty() || body_text.contains(&query) || meta.to_string().contains(&query) {
                    entries.push(json!({
                        "content": body_text,
                        "metadata": meta,
                    }));
                }
            }

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
    /// Create a new `MemoryWriteTool` for the given workspace.
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
            description: "Write a note to memory. Supports YAML frontmatter (---key: value---), @path references to inline file content, and quick append (starting with # appends a single line).".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "note": { "type": "string", "description": "Memory note content. Prefix with # for quick single-line append." }
                },
                "required": ["note"]
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

            // Expand @path references
            let expanded = store.expand_references(&note);

            let index_path = store.index_path();
            let mut content = String::new();
            if index_path.exists() {
                content = fs::read_to_string(&index_path)
                    .map_err(|e| ToolError::Execution(format!("read memory: {e}")))?;
                if !content.ends_with('\n') {
                    content.push('\n');
                }
                content.push_str("\n===\n");
            }

            if expanded.starts_with('#') {
                // Quick append: single line, no timestamp
                let line = expanded.trim_start_matches('#').trim();
                content.push_str(&format!("- {line}\n"));
                let display = line.to_string();
                fs::write(&index_path, &content)
                    .map_err(|e| ToolError::Execution(format!("write memory: {e}")))?;
                return Ok(ToolOutput::new(json!({
                    "written": true,
                    "entry": display,
                    "quick_append": true,
                })));
            }

            // Parse frontmatter from expanded note
            let (_meta, body) = parse_frontmatter(&expanded);
            let entry = format!(
                "---\nts: {ts}\n---\n{note}\n",
                ts = chrono_now(),
            );

            content.push_str(&entry);

            fs::write(&index_path, &content)
                .map_err(|e| ToolError::Execution(format!("write memory: {e}")))?;

            Ok(ToolOutput::new(json!({
                "written": true,
                "entry": body.trim(),
            })))
        })
    }
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    let (year, month, day, hour, min) = {
        let days = secs / 86400;
        let time_secs = secs % 86400;
        let h = time_secs / 3600;
        let m = (time_secs % 3600) / 60;

        let mut y = 1970i64;
        let mut remaining = days as i64;
        loop {
            let days_in_year = if is_leap(y) { 366 } else { 365 };
            if remaining < days_in_year {
                break;
            }
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
            if remaining < md {
                break;
            }
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
        assert!(entries[0]["content"].as_str().unwrap().contains("Rust"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_quick_append() {
        let dir = std::env::temp_dir().join(format!("joker-memory-qa-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let write_tool = MemoryWriteTool::new(&dir);

        let output = write_tool
            .call(ToolInvocation {
                call_id: "1".into(),
                name: ToolName::new("memory_write"),
                arguments: json!({"note": "# Quick line"}),
            })
            .await
            .unwrap();
        assert!(output.output["quick_append"].as_bool().unwrap_or(false));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_frontmatter_parsing() {
        let content = "---\ntype: project\ntags: [rust]\n---\nBody text";
        let (meta, body) = parse_frontmatter(content);
        assert_eq!(meta["type"], "project");
        assert_eq!(body.trim(), "Body text");
    }

    #[test]
    fn test_extract_lines() {
        let content = "a\nb\nc\nd\ne";
        assert_eq!(extract_lines(content, "L2-L4"), "b\nc\nd");
        assert_eq!(extract_lines(content, "L1-L1"), "a");
        assert_eq!(extract_lines(content, "L10"), "");
    }
}
