use std::fs;
use std::path::PathBuf;

use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput,
};
use serde::Deserialize;
use serde_json::json;

use crate::workspace::{WorkspaceTool, parse_args};

#[derive(Debug, Deserialize)]
struct PatchArgs {
    path: String,
    patch: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ApplyPatchTool {
    workspace: WorkspaceTool,
}

impl ApplyPatchTool {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            workspace: WorkspaceTool::new(root),
        }
    }

    fn apply_patch(content: &str, patch_str: &str) -> Result<String, ToolError> {
        let mut result = content.to_string();
        let mut patch_pos = 0usize;
        let patch_lines: Vec<&str> = patch_str.lines().collect();

        while patch_pos < patch_lines.len() {
            let line = patch_lines[patch_pos];

            if line.starts_with("--- ") || line.starts_with("+++ ") || line.is_empty() {
                patch_pos += 1;
                continue;
            }

            if let Some(header) = line.strip_prefix("@@ ") {
                if let Some(hunk_end) = header.find(" @@") {
                    let hunk_header = &header[..hunk_end];
                    let parts: Vec<&str> = hunk_header.split_whitespace().collect();
                    if parts.len() < 2 {
                        return Err(ToolError::InvalidArguments(format!(
                            "invalid hunk header: {line}"
                        )));
                    }
                    let old_start_str = parts[0].strip_prefix('-').unwrap_or(parts[0]);
                    let old_start: usize = old_start_str
                        .split(',')
                        .next()
                        .unwrap_or("1")
                        .parse()
                        .map_err(|_| {
                        ToolError::InvalidArguments(format!("invalid line number in hunk: {line}"))
                    })?;

                    patch_pos += 1;
                    let mut hunk_removals: Vec<String> = Vec::new();
                    let mut hunk_additions: Vec<String> = Vec::new();
                    let mut has_context = false;

                    while patch_pos < patch_lines.len() {
                        let hunk_line = patch_lines[patch_pos];
                        if hunk_line.starts_with("@@ ") {
                            break;
                        }
                        if let Some(stripped) = hunk_line.strip_prefix('-') {
                            hunk_removals.push(stripped.to_string());
                        } else if let Some(stripped) = hunk_line.strip_prefix('+') {
                            hunk_additions.push(stripped.to_string());
                        } else {
                            let ctx = if let Some(stripped) = hunk_line.strip_prefix(' ') {
                                stripped
                            } else {
                                hunk_line
                            };
                            hunk_removals.push(ctx.to_string());
                            hunk_additions.push(ctx.to_string());
                            has_context = true;
                        }
                        patch_pos += 1;
                    }

                    let content_line_idx = old_start.saturating_sub(1);
                    let current_lines: Vec<&str> = result.lines().collect();

                    if content_line_idx >= current_lines.len() {
                        if !hunk_additions.is_empty() {
                            result.push('\n');
                            result.push_str(&hunk_additions.join("\n"));
                        }
                        continue;
                    }

                    let removal_text = hunk_removals.join("\n");
                    let addition_text = hunk_additions.join("\n");

                    let line_count = hunk_removals.len();
                    let end = std::cmp::min(content_line_idx + line_count, current_lines.len());
                    let search_section = current_lines[content_line_idx..end].join("\n");

                    if search_section == removal_text || (!has_context && removal_text.is_empty()) {
                        let before: Vec<&str> = current_lines[..content_line_idx].to_vec();
                        let after: Vec<&str> =
                            if content_line_idx + line_count < current_lines.len() {
                                current_lines[content_line_idx + line_count..].to_vec()
                            } else {
                                Vec::new()
                            };

                        let mut new_lines = before;
                        if !addition_text.is_empty() {
                            for add_line in hunk_additions.iter() {
                                new_lines.push(add_line);
                            }
                        }
                        new_lines.extend(after);

                        result = new_lines.join("\n");
                    } else if let Some(pos) = result.find(&removal_text)
                        && !removal_text.is_empty()
                    {
                        result.replace_range(pos..pos + removal_text.len(), &addition_text);
                    }
                }
            } else {
                patch_pos += 1;
            }
        }

        Ok(result)
    }
}

impl Tool for ApplyPatchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("apply_patch"),
            description: "Apply a unified diff patch to a workspace file.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative file path to patch." },
                    "patch": { "type": "string", "description": "Unified diff patch content." }
                },
                "required": ["path", "patch"]
            }),
            annotations: ToolAnnotations::from_capabilities(
                ToolExecution::Sequential,
                vec![ToolCapability::WritesFiles],
                None,
                ApprovalRequirement::Required,
            ),
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async move {
            let args = parse_args::<PatchArgs>(invocation.arguments)?;
            let path = self.workspace.resolve_write(&args.path)?;

            let content = if path.exists() {
                fs::read_to_string(&path)
                    .map_err(|error| ToolError::Execution(format!("read failed: {error}")))?
            } else {
                String::new()
            };

            let patched = Self::apply_patch(&content, &args.patch)?;

            fs::write(&path, &patched)
                .map_err(|error| ToolError::Execution(format!("write failed: {error}")))?;

            Ok(ToolOutput::new(json!({
                "path": args.path,
                "applied": true,
            })))
        })
    }
}
