use std::fs;
use std::path::PathBuf;

use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput,
};
use serde::Deserialize;
use serde_json::json;

use crate::workspace::{parse_args, WorkspaceTool};

#[derive(Debug, Deserialize)]
struct PathArgs {
    path: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ListFilesTool {
    workspace: WorkspaceTool,
}

impl ListFilesTool {
    pub(crate) fn new(root: PathBuf) -> Self {
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
                capabilities: vec![ToolCapability::ReadOnly],
                default_approval: ApprovalRequirement::Auto,
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async move {
            let args = parse_args::<PathArgs>(invocation.arguments)?;
            let path = self
                .workspace
                .resolve_read(args.path.as_deref().unwrap_or("."))?;
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
