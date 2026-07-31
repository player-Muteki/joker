use std::fs;
use std::path::PathBuf;
use tracing::*;

use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput,
};
use serde::Deserialize;
use serde_json::json;

use crate::workspace::{WorkspaceTool, parse_args};

/// UTF-8 BOM bytes.
const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// Line-ending styles detected in existing file content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineEnding {
    Lf,
    CrLf,
    Mixed,
}

fn detect_line_ending(content: &[u8]) -> LineEnding {
    let has_crlf = content.windows(2).any(|w| w == b"\r\n");
    let has_bare_lf = content.contains(&b'\n') && !content.windows(2).any(|w| w == b"\r\n");
    match (has_crlf, has_bare_lf) {
        (true, false) => LineEnding::CrLf,
        (false, true) => LineEnding::Lf,
        (true, true) => LineEnding::Mixed,
        (false, false) => LineEnding::Lf, // no newlines at all
    }
}

/// Normalize newlines in `content` to match the target line ending.
fn normalize_newlines(content: &str, target: LineEnding) -> String {
    if target == LineEnding::CrLf {
        // Convert bare LF to CRLF, but preserve existing CRLF
        content.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        // Strip CR before LF to convert CRLF → LF
        content.replace("\r\n", "\n")
    }
}

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
            description: "Create or overwrite a UTF-8 text file within the workspace. Preserves BOM and line endings.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative file path." },
                    "content": { "type": "string", "description": "File content to write." }
                },
                "required": ["path", "content"]
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
            let args = parse_args::<WriteFileArgs>(invocation.arguments)?;
            let path = self.workspace.resolve_write(&args.path)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    ToolError::Execution(format!("create dirs failed: {error}"))
                })?;
            }

            let mut content = args.content.into_bytes();

            // ── BOM preservation ───────────────────────────────────────
            // If the existing file has a UTF-8 BOM, prepend it to the new content.
            // Reference: pi's file format detection in computeFileLists.
            let has_bom = path.exists() && {
                let existing = fs::read(&path).unwrap_or_default();
                existing.starts_with(UTF8_BOM)
            };
            if has_bom && !content.starts_with(UTF8_BOM) {
                let mut prefixed = UTF8_BOM.to_vec();
                prefixed.append(&mut content);
                content = prefixed;
            }

            // ── Newline preservation ───────────────────────────────────
            // Detect the existing file's line ending style and normalize.
            // Reference: gemini-cli's content-aware shell output handling.
            if path.exists() {
                let existing = fs::read(&path).unwrap_or_default();
                let detected = detect_line_ending(&existing);
                let content_str = String::from_utf8_lossy(&content);
                let normalized = normalize_newlines(&content_str, detected);
                content = normalized.into_bytes();
            }

            // ── Stale check ────────────────────────────────────────────
            // If the file exists and was modified since the model last read it
            // (approximate: content differs from what we're writing), warn in output.
            // Reference: claude-code's file edit diff preview.
            let stale = if path.exists() {
                let existing = fs::read(&path).unwrap_or_default();
                if existing == content {
                    None
                } else {
                    info!(target: "tool.write_file", path = %args.path, "file content differs from existing — overwriting");
                    Some(())
                }
            } else {
                None
            };

            info!(target: "tool.write_file", path = %args.path, content_len = content.len(), bom = has_bom, "writing file");
            fs::write(&path, &content)
                .map_err(|error| ToolError::Execution(format!("write failed: {error}")))?;

            let mut result = json!({
                "path": args.path,
                "size": content.len(),
                "bom_preserved": has_bom,
            });
            if stale.is_some() {
                result["stale_overwrite"] = json!(true);
            }
            Ok(ToolOutput::new(result))
        })
    }
}
