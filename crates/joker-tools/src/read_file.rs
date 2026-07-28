use std::fs;
use std::path::{Path, PathBuf};

use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput,
};
use serde::Deserialize;
use serde_json::json;

use crate::workspace::{parse_args, truncate_at_char_boundary, detect_line_ending, WorkspaceTool};

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico"];
const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10 MB

#[derive(Debug, Deserialize)]
struct ReadFileArgs {
    path: String,
    max_bytes: Option<usize>,
    offset: Option<usize>,
    limit: Option<usize>,
    code_fence: Option<bool>,
    /// If true, allow reading binary files (return base64). Default false.
    allow_binary: Option<bool>,
}

fn likely_binary(data: &[u8]) -> bool {
    let check_len = data.len().min(8192);
    let null_count = data[..check_len].iter().filter(|&&b| b == 0).count();
    if null_count > 0 {
        return true;
    }
    let non_text = data[..check_len]
        .iter()
        .filter(|&&b| b < 8 || (b > 13 && b < 32 && b != 27 && b != 10)) // not \n, \r, \t, \b, ESC
        .count();
    non_text > check_len / 4
}

#[derive(Clone, Debug)]
pub(crate) struct ReadFileTool {
    workspace: WorkspaceTool,
}

impl ReadFileTool {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            workspace: WorkspaceTool::new(root),
        }
    }

    fn is_image(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| IMAGE_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
            .unwrap_or(false)
    }
}

impl Tool for ReadFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("read_file"),
            description: "Read a file from the workspace. Supports text files (UTF-8) with optional line offset/limit, and image files (returned as base64 data URL).".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "max_bytes": { "type": "integer", "minimum": 1, "maximum": 200000 },
                    "offset": { "type": "integer", "minimum": 1, "description": "1-based line number to start reading from." },
                    "limit": { "type": "integer", "minimum": 1, "description": "Maximum number of lines to return." },
                    "code_fence": { "type": "boolean", "description": "Wrap content in a code fence with language hint." },
                    "allow_binary": { "type": "boolean", "description": "Allow reading binary files (returns base64). Default false." }
                },
                "required": ["path"]
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
            let args = parse_args::<ReadFileArgs>(invocation.arguments)?;
            let path = self.workspace.resolve_read(&args.path)?;
            let allow_binary = args.allow_binary.unwrap_or(false);

            let metadata = fs::metadata(&path)
                .map_err(|error| ToolError::Execution(format!("metadata failed: {error}")))?;
            let file_size = metadata.len();

            if file_size > MAX_FILE_SIZE && !metadata.is_dir() {
                return Err(ToolError::InvalidArguments(format!(
                    "File too large: {size} bytes (max: {max}). \
                     Use `offset`/`limit` to read portions, or grep/glob to search.",
                    size = file_size, max = MAX_FILE_SIZE
                )));
            }

            if Self::is_image(&path) {
                let bytes = fs::read(&path)
                    .map_err(|error| ToolError::Execution(error.to_string()))?;
                let b64 = base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &bytes,
                );
                let ext = path.extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("png")
                    .to_lowercase();
                let data_url = format!("data:image/{ext};base64,{b64}");
                return Ok(ToolOutput::new(json!({
                    "path": args.path,
                    "content": data_url,
                    "mime": format!("image/{ext}"),
                    "size": bytes.len(),
                    "binary": true,
                })));
            }

            let raw = fs::read(&path)
                .map_err(|error| ToolError::Execution(error.to_string()))?;

            let is_binary = likely_binary(&raw);

            if is_binary {
                if !allow_binary {
                    return Err(ToolError::InvalidArguments(format!(
                        "File appears to be binary ({} bytes). \
                         Use `allow_binary: true` to read as base64, or grep/glob to search.",
                        raw.len()
                    )));
                }
                let b64 = base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &raw,
                );
                return Ok(ToolOutput::new(json!({
                    "path": args.path,
                    "content": b64,
                    "size": raw.len(),
                    "binary": true,
                    "encoding": "base64",
                })));
            }

            let content = String::from_utf8(raw)
                .map_err(|_| ToolError::InvalidArguments(
                    "File is not valid UTF-8 text.".into()
                ))?;

            let has_bom = content.starts_with('\u{FEFF}');
            let (bom, text) = if has_bom { ("\u{FEFF}", &content[3..]) } else { ("", content.as_str()) };

            let max_bytes = args.max_bytes.unwrap_or(64_000).min(200_000);
            let line_ending = detect_line_ending(text);
            let line_ending_name = if line_ending == "\r\n" { "crlf" } else { "lf" };

            let lines: Vec<&str> = text.lines().collect();
            let total_lines = lines.len();
            let start_line = args.offset.unwrap_or(1).saturating_sub(1);
            let end_line = args.limit.map(|limit| start_line + limit).unwrap_or(total_lines);
            let selected = if start_line > 0 || end_line < total_lines {
                if start_line >= total_lines {
                    return Ok(ToolOutput::new(json!({
                        "path": args.path,
                        "content": String::new(),
                        "line_count": total_lines,
                        "offset": args.offset.unwrap_or(1),
                        "limit": args.limit,
                        "truncated": false,
                        "line_ending": line_ending_name,
                        "bom": has_bom,
                    })));
                }
                let mut selected = lines[start_line..end_line.min(total_lines)].join(line_ending);
                if end_line < total_lines {
                    selected.push_str(&format!(
                        "\n... (showing lines {}-{} of {})",
                        start_line + 1, end_line, total_lines
                    ));
                }
                selected
            } else {
                text.to_string()
            };

            let truncated = selected.len() > max_bytes;
            let text_out = if truncated {
                let t = truncate_at_char_boundary(&selected, max_bytes).to_string();
                if bom.is_empty() { t } else { format!("{bom}{t}") }
            } else {
                if bom.is_empty() { selected } else { format!("{bom}{selected}") }
            };

            let mut result = json!({
                "path": args.path,
                "content": text_out,
                "line_count": total_lines,
                "offset": args.offset.unwrap_or(1),
                "limit": args.limit,
                "truncated": truncated,
                "line_ending": line_ending_name,
                "bom": has_bom,
            });

            if args.code_fence.unwrap_or(false)
                && let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    result["content"] = json!(format!("```{}\n{}\n```", ext, text_out));
                }

            Ok(ToolOutput::new(result))
        })
    }
}
