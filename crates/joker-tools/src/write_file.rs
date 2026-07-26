use std::fs;
use std::path::PathBuf;
use tracing::*;

use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput,
};
use serde::Deserialize;
use serde_json::json;

use crate::workspace::{parse_args, WorkspaceTool};

#[derive(Debug, Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

#[derive(Clone, Debug)]
pub(crate) struct WriteFileTool {
    workspace: WorkspaceTool,
}

impl WriteFileTool {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            workspace: WorkspaceTool::new(root),
        }
    }
}

impl Tool for WriteFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("write_file"),
            description: "Create or overwrite a UTF-8 text file within the workspace.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative file path." },
                    "content": { "type": "string", "description": "File content to write." }
                },
                "required": ["path", "content"]
            }),
            annotations: ToolAnnotations {
                execution: ToolExecution::Sequential,
                mutating: true,
                timeout: None,
                capabilities: vec![ToolCapability::WritesFiles],
                default_approval: ApprovalRequirement::Required,
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async move {
            let args = parse_args::<WriteFileArgs>(invocation.arguments)?;
            let path = self.workspace.resolve_write(&args.path)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| ToolError::Execution(format!("create dirs failed: {error}")))?;
            }
            info!(target: "tool.write_file", path = %args.path, content_len = args.content.len(), "writing file");
            fs::write(&path, &args.content)
                .map_err(|error| ToolError::Execution(format!("write failed: {error}")))?;
            Ok(ToolOutput::new(json!({
                "path": args.path,
                "size": args.content.len(),
            })))
        })
    }
}
