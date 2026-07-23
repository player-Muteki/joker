#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use joker::{
    Tool, ToolAnnotations, ToolDefinition, ToolError, ToolExecution, ToolFuture, ToolInvocation,
    ToolName, ToolOutput, ToolRegistry,
};
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToolsError {
    #[error("tool registry error: {0}")]
    Registry(#[from] ToolError),
    #[error("workspace path error: {0}")]
    Workspace(std::io::Error),
}

pub fn readonly_tools(workspace: impl Into<PathBuf>) -> Result<Arc<ToolRegistry>, ToolsError> {
    Ok(Arc::new(readonly_tool_registry(workspace)?))
}

pub fn readonly_tool_registry(workspace: impl Into<PathBuf>) -> Result<ToolRegistry, ToolsError> {
    let workspace = workspace.into();
    let mut registry = ToolRegistry::new();
    registry.insert(ListFilesTool::new(workspace.clone()))?;
    registry.insert(ReadFileTool::new(workspace.clone()))?;
    registry.insert(GrepTool::new(workspace))?;
    Ok(registry)
}

#[derive(Clone, Debug)]
struct WorkspaceTool {
    root: PathBuf,
}

impl WorkspaceTool {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, ToolError> {
        let root = fs::canonicalize(&self.root)
            .map_err(|error| ToolError::Execution(format!("workspace does not exist: {error}")))?;
        let candidate = root.join(path.trim_start_matches('/'));
        let resolved = fs::canonicalize(&candidate).map_err(|error| {
            ToolError::Execution(format!("path does not exist: {path}: {error}"))
        })?;
        if !resolved.starts_with(&root) {
            return Err(ToolError::InvalidArguments(format!(
                "path escapes workspace: {path}"
            )));
        }
        Ok(resolved)
    }
}

#[derive(Clone, Debug)]
struct ListFilesTool {
    workspace: WorkspaceTool,
}

impl ListFilesTool {
    fn new(root: PathBuf) -> Self {
        Self {
            workspace: WorkspaceTool::new(root),
        }
    }
}

impl Tool for ListFilesTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("list_files"),
            description: "List files in a workspace directory.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative directory path." }
                }
            }),
            annotations: ToolAnnotations {
                execution: ToolExecution::ParallelSafe,
                mutating: false,
                timeout: None,
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async move {
            let args = parse_args::<PathArgs>(invocation.arguments)?;
            let path = self
                .workspace
                .resolve(args.path.as_deref().unwrap_or("."))?;
            let entries = fs::read_dir(&path)
                .map_err(|error| ToolError::Execution(error.to_string()))?
                .map(|entry| {
                    let entry = entry.map_err(|error| ToolError::Execution(error.to_string()))?;
                    let kind = entry
                        .file_type()
                        .map_err(|error| ToolError::Execution(error.to_string()))?;
                    Ok(json!({
                        "name": entry.file_name().to_string_lossy(),
                        "kind": if kind.is_dir() { "dir" } else { "file" },
                    }))
                })
                .collect::<Result<Vec<_>, ToolError>>()?;
            Ok(ToolOutput::new(json!({ "entries": entries })))
        })
    }
}

#[derive(Clone, Debug)]
struct ReadFileTool {
    workspace: WorkspaceTool,
}

impl ReadFileTool {
    fn new(root: PathBuf) -> Self {
        Self {
            workspace: WorkspaceTool::new(root),
        }
    }
}

impl Tool for ReadFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("read_file"),
            description: "Read a UTF-8 text file from the workspace.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "max_bytes": { "type": "integer", "minimum": 1, "maximum": 200000 }
                },
                "required": ["path"]
            }),
            annotations: ToolAnnotations {
                execution: ToolExecution::ParallelSafe,
                mutating: false,
                timeout: None,
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async move {
            let args = parse_args::<ReadFileArgs>(invocation.arguments)?;
            let path = self.workspace.resolve(&args.path)?;
            let content = fs::read_to_string(&path)
                .map_err(|error| ToolError::Execution(error.to_string()))?;
            let max_bytes = args.max_bytes.unwrap_or(64_000).min(200_000);
            let truncated = content.len() > max_bytes;
            let text = if truncated {
                truncate_at_char_boundary(&content, max_bytes).to_string()
            } else {
                content
            };
            Ok(ToolOutput::new(json!({
                "path": args.path,
                "content": text,
                "truncated": truncated,
            })))
        })
    }
}

#[derive(Clone, Debug)]
struct GrepTool {
    workspace: WorkspaceTool,
}

impl GrepTool {
    fn new(root: PathBuf) -> Self {
        Self {
            workspace: WorkspaceTool::new(root),
        }
    }
}

impl Tool for GrepTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("grep"),
            description: "Search UTF-8 workspace files for a substring.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "path": { "type": "string" },
                    "max_matches": { "type": "integer", "minimum": 1, "maximum": 200 }
                },
                "required": ["query"]
            }),
            annotations: ToolAnnotations {
                execution: ToolExecution::ParallelSafe,
                mutating: false,
                timeout: None,
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async move {
            let args = parse_args::<GrepArgs>(invocation.arguments)?;
            if args.query.is_empty() {
                return Err(ToolError::InvalidArguments("query cannot be empty".into()));
            }
            let root = self
                .workspace
                .resolve(args.path.as_deref().unwrap_or("."))?;
            let max_matches = args.max_matches.unwrap_or(50).min(200);
            let mut matches = Vec::new();
            grep_path(&root, &root, &args.query, max_matches, &mut matches)?;
            Ok(ToolOutput::new(json!({ "matches": matches })))
        })
    }
}

fn grep_path(
    root: &Path,
    path: &Path,
    query: &str,
    max_matches: usize,
    matches: &mut Vec<Value>,
) -> Result<(), ToolError> {
    if matches.len() >= max_matches {
        return Ok(());
    }
    let metadata = fs::metadata(path).map_err(|error| ToolError::Execution(error.to_string()))?;
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|error| ToolError::Execution(error.to_string()))? {
            let entry = entry.map_err(|error| ToolError::Execution(error.to_string()))?;
            grep_path(root, &entry.path(), query, max_matches, matches)?;
            if matches.len() >= max_matches {
                break;
            }
        }
        return Ok(());
    }
    if metadata.len() > 1_000_000 {
        return Ok(());
    }
    let Ok(content) = fs::read_to_string(path) else {
        return Ok(());
    };
    for (index, line) in content.lines().enumerate() {
        if line.contains(query) {
            matches.push(json!({
                "path": path.strip_prefix(root).unwrap_or(path).to_string_lossy(),
                "line": index + 1,
                "text": line,
            }));
            if matches.len() >= max_matches {
                break;
            }
        }
    }
    Ok(())
}

fn parse_args<T>(value: Value) -> Result<T, ToolError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(value).map_err(|error| ToolError::InvalidArguments(error.to_string()))
}

fn truncate_at_char_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut boundary = 0;
    for (index, _) in text.char_indices() {
        if index > max_bytes {
            break;
        }
        boundary = index;
    }
    &text[..boundary]
}

#[derive(Debug, Deserialize)]
struct PathArgs {
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReadFileArgs {
    path: String,
    max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct GrepArgs {
    query: String,
    path: Option<String>,
    max_matches: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use joker::ToolInvocation;

    #[tokio::test]
    async fn read_file_is_workspace_scoped() {
        let root = std::env::temp_dir().join(format!("joker-tools-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), "hello").unwrap();
        let registry = readonly_tools(&root).unwrap();

        let result = registry
            .call(ToolInvocation {
                call_id: "1".into(),
                name: ToolName::new("read_file"),
                arguments: json!({"path":"a.txt"}),
            })
            .await;

        let _ = fs::remove_dir_all(&root);
        assert!(!result.is_error);
        assert_eq!(result.output["content"], "hello");
    }

    #[tokio::test]
    async fn grep_finds_matches() {
        let root = std::env::temp_dir().join(format!("joker-tools-grep-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), "alpha\nbeta").unwrap();
        let registry = readonly_tools(&root).unwrap();

        let result = registry
            .call(ToolInvocation {
                call_id: "1".into(),
                name: ToolName::new("grep"),
                arguments: json!({"query":"beta"}),
            })
            .await;

        let _ = fs::remove_dir_all(&root);
        assert!(!result.is_error);
        assert_eq!(result.output["matches"][0]["line"], 2);
    }
}
