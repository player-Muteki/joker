use std::path::PathBuf;

use ignore::WalkBuilder;
use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput,
};
use serde::Deserialize;
use serde_json::json;

use crate::workspace::{WorkspaceTool, parse_args};

#[derive(Debug, Deserialize)]
struct PathArgs {
    path: Option<String>,
    recursive: Option<bool>,
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
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative directory path. Defaults to workspace root."
                    },
                    "recursive": {
                        "type": "boolean",
                        "description": "When true (default), list all files recursively. When false, only direct children."
                    }
                }
            }),
            annotations: ToolAnnotations::from_capabilities(
                ToolExecution::ParallelSafe,
                vec![ToolCapability::ReadOnly],
                None,
                ApprovalRequirement::Auto,
            ),
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async move {
            let args = parse_args::<PathArgs>(invocation.arguments)?;
            let path = self
                .workspace
                .resolve_read(args.path.as_deref().unwrap_or("."))?;
            let recursive = args.recursive.unwrap_or(true);

            let root = self.workspace.root.clone();

            let entries = if recursive {
                let mut walk = WalkBuilder::new(&path);
                walk.git_ignore(false)
                    .git_global(false)
                    .git_exclude(false)
                    .hidden(false)
                    .max_depth(None);
                let mut items = Vec::new();
                for result in walk.build() {
                    let entry = result.map_err(|error| ToolError::Execution(error.to_string()))?;
                    let relative = entry
                        .path()
                        .strip_prefix(&root)
                        .unwrap_or(entry.path())
                        .to_string_lossy()
                        .to_string();
                    let kind = entry
                        .file_type()
                        .map(|ft| if ft.is_dir() { "dir" } else { "file" })
                        .unwrap_or("file");
                    items.push(json!({
                        "name": relative,
                        "kind": kind,
                    }));
                }
                items
            } else {
                let dir = std::fs::read_dir(&path)
                    .map_err(|error| ToolError::Execution(error.to_string()))?;
                dir.map(|entry| {
                    let entry = entry.map_err(|error| ToolError::Execution(error.to_string()))?;
                    let name = entry.file_name().to_string_lossy().to_string();
                    let kind = entry
                        .file_type()
                        .map_err(|error| ToolError::Execution(error.to_string()))?;
                    Ok(json!({
                        "name": name,
                        "kind": if kind.is_dir() { "dir" } else { "file" },
                    }))
                })
                .collect::<Result<Vec<_>, ToolError>>()?
            };

            Ok(ToolOutput::new(json!({ "entries": entries })))
        })
    }
}
