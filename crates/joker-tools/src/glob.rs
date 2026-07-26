//! Glob pattern matching tool with .gitignore awareness.
//!
//! Wraps the `ignore` crate to search for files matching a glob pattern,
//! respecting `.gitignore` rules and configurable recursion depth.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput,
};
use serde::Deserialize;
use serde_json::json;

use crate::workspace::{WorkspaceTool, parse_args};

/// Standalone API: find files matching a glob pattern within a workspace.
pub async fn glob_files(
    root: &Path,
    pattern: &str,
    max_depth: Option<usize>,
    max_results: Option<usize>,
) -> Result<Vec<PathBuf>, ToolError> {
    let root = std::fs::canonicalize(root)
        .map_err(|error| ToolError::Execution(format!("workspace does not exist: {error}")))?;

    let mut builder = WalkBuilder::new(&root);
    builder
        .git_ignore(true)
        .git_global(true)
        .hidden(false)
        .follow_links(false)
        .max_depth(Some(max_depth.map(|d| d + 1).unwrap_or(10)));

    let glob = glob::Pattern::new(pattern)
        .map_err(|e| ToolError::InvalidArguments(format!("invalid glob pattern: {e}")))?;

    let mut results: Vec<PathBuf> = Vec::new();
    let max_results = max_results.unwrap_or(100);

    for entry in builder.build() {
        if results.len() >= max_results {
            break;
        }
        let entry = entry.map_err(|error| ToolError::Execution(error.to_string()))?;
        if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
            continue;
        }
        let relative = entry.path().strip_prefix(&root).map_err(|error| {
            ToolError::Execution(format!("path strip: {error}"))
        })?;

        if glob.matches_path(relative) {
            results.push(relative.to_path_buf());
        }
    }

    Ok(results)
}

#[derive(Clone, Debug)]
pub struct GlobTool {
    workspace: WorkspaceTool,
}

impl GlobTool {
    pub fn new(root: PathBuf) -> Self {
        Self {
            workspace: WorkspaceTool::new(root),
        }
    }
}

impl Tool for GlobTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("glob"),
            description: "Find files matching a glob pattern (e.g. \"**/*.rs\"). Respects .gitignore.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern to match file paths against."
                    },
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative directory to search from. Defaults to workspace root."
                    },
                    "max_depth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20,
                        "description": "Maximum recursion depth."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 500,
                        "default": 100,
                        "description": "Maximum number of results to return."
                    }
                },
                "required": ["pattern"]
            }),
            annotations: ToolAnnotations {
                execution: ToolExecution::ParallelSafe,
                mutating: false,
                timeout: None,
                capabilities: vec![ToolCapability::ReadOnly],
                default_approval: ApprovalRequirement::Auto,
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async move {
            let args = parse_args::<GlobArgs>(invocation.arguments)?;
            let root = args
                .path
                .as_deref()
                .map(|p| self.workspace.resolve_read(p))
                .unwrap_or_else(|| Ok(self.workspace.root.clone()))?;

            let results = glob_files(&root, &args.pattern, args.max_depth, args.max_results).await?;
            let paths: Vec<String> = results.iter().map(|p| p.to_string_lossy().to_string()).collect();

            Ok(ToolOutput::new(json!({
                "pattern": args.pattern,
                "count": paths.len(),
                "paths": paths,
            })))
        })
    }
}

#[derive(Debug, Deserialize)]
struct GlobArgs {
    pattern: String,
    path: Option<String>,
    max_depth: Option<usize>,
    max_results: Option<usize>,
}
