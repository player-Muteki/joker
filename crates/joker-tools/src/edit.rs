use std::fs;
use std::path::{Path, PathBuf};

use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput,
};
use serde::Deserialize;
use serde_json::json;

use crate::workspace::{parse_args, detect_line_ending, normalize_line_endings, WorkspaceTool};

/// Standalone API: apply a string replacement to a file.
pub fn edit_file(path: &Path, old_string: &str, new_string: &str, replace_all: bool) -> Result<String, ToolError> {
    let content = fs::read_to_string(path)
        .map_err(|error| ToolError::Execution(format!("read failed: {error}")))?;

    let line_ending = detect_line_ending(&content);
    let normalized_new = normalize_line_endings(new_string, line_ending);

    let count = content.matches(old_string).count();
    if count == 0 {
        return Err(ToolError::InvalidArguments("old_string not found in file".into()));
    }
    if count > 1 && !replace_all {
        return Err(ToolError::InvalidArguments(format!(
            "old_string found {count} times in file. Use replace_all=true to replace all occurrences, or provide more surrounding context to make the match unique."
        )));
    }

    let new_content = if replace_all {
        content.replace(old_string, &normalized_new)
    } else {
        content.replacen(old_string, &normalized_new, 1)
    };

    let current = fs::read_to_string(path)
        .map_err(|error| ToolError::Execution(format!("stale check read: {error}")))?;
    if current != content {
        return Err(ToolError::Execution("file changed between read and write — re-read and try again".into()));
    }

    fs::write(path, &new_content)
        .map_err(|error| ToolError::Execution(format!("write failed: {error}")))?;

    Ok(new_content)
}

#[derive(Debug, Deserialize)]
struct EditFileArgs {
    path: String,
    old_string: String,
    new_string: String,
    replace_all: Option<bool>,
}

#[derive(Clone, Debug)]
pub(crate) struct EditFileTool {
    workspace: WorkspaceTool,
}

impl EditFileTool {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            workspace: WorkspaceTool::new(root),
        }
    }
}

impl Tool for EditFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("edit_file"),
            description: "Replace text in a file. old_string must match exactly. Use replace_all=true to replace all occurrences.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative file path." },
                    "old_string": { "type": "string", "description": "The exact text to replace." },
                    "new_string": { "type": "string", "description": "The replacement text." },
                    "replace_all": { "type": "boolean", "description": "When true, replace all occurrences. Default: false." }
                },
                "required": ["path", "old_string", "new_string"]
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
            let args = parse_args::<EditFileArgs>(invocation.arguments)?;
            let path = self.workspace.resolve_read(&args.path)?;
            let replace_all = args.replace_all.unwrap_or(false);

            let _new_content = edit_file(&path, &args.old_string, &args.new_string, replace_all)?;

            Ok(ToolOutput::new(json!({
                "path": args.path,
                "replaced": true,
                "replace_all": replace_all,
            })))
        })
    }
}
