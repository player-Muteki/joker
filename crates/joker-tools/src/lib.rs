#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::AsyncReadExt;

pub mod fetch_url;
pub mod glob;
pub mod memory;
pub mod todo;
pub mod web_search;

use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput, ToolRegistry,
};
use fetch_url::FetchUrlTool;
use glob::GlobTool;
use joker::WebSearch;
use todo::TodoWriteTool;
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToolsError {
    #[error("tool registry error: {0}")]
    Registry(#[from] ToolError),
    #[error("workspace path error: {0}")]
    Workspace(std::io::Error),
    #[error("execution error: {0}")]
    ExecutionError(String),
}

pub fn readonly_tools(workspace: impl Into<PathBuf>) -> Result<Arc<ToolRegistry>, ToolsError> {
    Ok(Arc::new(readonly_tool_registry(workspace)?))
}

pub fn readonly_tool_registry(workspace: impl Into<PathBuf>) -> Result<ToolRegistry, ToolsError> {
    let workspace = workspace.into();
    let mut registry = ToolRegistry::new();
    registry.insert(ListFilesTool::new(workspace.clone()))?;
    registry.insert(ReadFileTool::new(workspace.clone()))?;
    registry.insert(GrepTool::new(workspace.clone()))?;
    registry.insert(GlobTool::new(workspace.clone()))?;
    Ok(registry)
}

/// Create a registry with write-capable tools (write_file, edit_file, shell)
/// alongside the read-only tools.
pub fn writeable_tools(workspace: impl Into<PathBuf>) -> Result<Arc<ToolRegistry>, ToolsError> {
    Ok(Arc::new(writeable_tool_registry(workspace)?))
}

pub fn writeable_tool_registry(
    workspace: impl Into<PathBuf>,
) -> Result<ToolRegistry, ToolsError> {
    let workspace = workspace.into();
    let mut registry = readonly_tool_registry(workspace.clone())?;
    registry.insert(WriteFileTool::new(workspace.clone()))?;
    registry.insert(EditFileTool::new(workspace.clone()))?;
    registry.insert(ShellTool::new(workspace.clone()))?;
    registry.insert(ApplyPatchTool::new(workspace.clone()))?;
    registry.insert(FetchUrlTool::new())?;
    registry.insert(TodoWriteTool::new(workspace.clone()))?;
    Ok(registry)
}

/// Create a registry with all built-in tools (read, write, network).
pub fn all_tools(workspace: impl Into<PathBuf>) -> Result<Arc<ToolRegistry>, ToolsError> {
    Ok(Arc::new(all_tool_registry(workspace)?))
}

pub fn all_tool_registry(
    workspace: impl Into<PathBuf>,
) -> Result<ToolRegistry, ToolsError> {
    let workspace = workspace.into();
    let mut registry = writeable_tool_registry(workspace.clone())?;

    // Web search tool — DuckDuckGo backend (free, no API key needed)
    match web_search::DuckDuckGoSearch::new() {
        Ok(backend) => {
            registry.insert(web_search::WebSearchTool::new(
                std::sync::Arc::new(backend) as std::sync::Arc<dyn WebSearch>
            ))?;
        }
        Err(e) => {
            eprintln!("warning: failed to initialize web search: {e}");
        }
    }

    // Memory tools
    registry.insert(memory::MemoryReadTool::new(workspace.clone()))?;
    registry.insert(memory::MemoryWriteTool::new(workspace.clone()))?;

    Ok(registry)
}

// ── Workspace path helper ───────────────────────────────────────────────

#[derive(Clone, Debug)]
pub(crate) struct WorkspaceTool {
    pub(crate) root: PathBuf,
}

impl WorkspaceTool {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Resolve a path for reading — the path must exist and be within the workspace.
    pub(crate) fn resolve_read(&self, path: &str) -> Result<PathBuf, ToolError> {
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

    /// Resolve a path for writing — the parent directory must exist and be within workspace.
    /// The target file may not exist yet (it will be created).
    pub(crate) fn resolve_write(&self, path: &str) -> Result<PathBuf, ToolError> {
        let root = fs::canonicalize(&self.root)
            .map_err(|error| ToolError::Execution(format!("workspace does not exist: {error}")))?;
        let candidate = root.join(path.trim_start_matches('/'));

        // Validate parent directory is within workspace
        let parent = candidate.parent().ok_or_else(|| {
            ToolError::InvalidArguments(format!("invalid path: {path}"))
        })?;

        // Canonicalize the parent (must exist) to check workspace boundary
        let resolved_parent = fs::canonicalize(parent).map_err(|error| {
            ToolError::Execution(format!("parent directory does not exist: {}: {error}", parent.display()))
        })?;

        if !resolved_parent.starts_with(&root) {
            return Err(ToolError::InvalidArguments(format!(
                "path escapes workspace: {path}"
            )));
        }

        Ok(candidate)
    }
}

// ── ListFilesTool ───────────────────────────────────────────────────────

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

// ── ReadFileTool ────────────────────────────────────────────────────────

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico"];

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
                    "limit": { "type": "integer", "minimum": 1, "description": "Maximum number of lines to return." }
                },
                "required": ["path"]
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
            let args = parse_args::<ReadFileArgs>(invocation.arguments)?;
            let path = self.workspace.resolve_read(&args.path)?;

            // Check if file is an image
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
                })));
            }

            // Text file: read content
            let content = fs::read_to_string(&path)
                .map_err(|error| ToolError::Execution(error.to_string()))?;

            // Detect BOM and strip for processing, but remember it
            let (bom, text) = if let Some(stripped) = content.strip_prefix('\u{FEFF}') {
                ("\u{FEFF}", stripped)
            } else {
                ("", content.as_str())
            };

            let max_bytes = args.max_bytes.unwrap_or(64_000).min(200_000);
            let line_ending = detect_line_ending(text);

            // Apply offset/limit (1-based line numbers)
            let lines: Vec<&str> = text.lines().collect();
            let total_lines = lines.len();
            let start_line = args.offset.unwrap_or(1).saturating_sub(1);
            let end_line = args.limit.map(|limit| start_line + limit).unwrap_or(total_lines);
            let selected = if start_line > 0 || end_line < total_lines {
                if start_line >= total_lines {
                    // offset past end: return empty but indicate total
                    return Ok(ToolOutput::new(json!({
                        "path": args.path,
                        "content": String::new(),
                        "line_count": total_lines,
                        "offset": args.offset.unwrap_or(1),
                        "limit": args.limit,
                        "truncated": false,
                    })));
                }
                lines[start_line..end_line.min(total_lines)].join(line_ending)
                    + if end_line < total_lines { "\n... (showing lines {}-{} of {})" } else { "" }
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

            Ok(ToolOutput::new(json!({
                "path": args.path,
                "content": text_out,
                "line_count": total_lines,
                "offset": args.offset.unwrap_or(1),
                "limit": args.limit,
                "truncated": truncated,
            })))
        })
    }
}

// ── GrepTool ────────────────────────────────────────────────────────────

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
            description: "Search workspace files for a pattern. Uses ripgrep if available, with fallback to a pure-Rust implementation.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "path": { "type": "string" },
                    "max_matches": { "type": "integer", "minimum": 1, "maximum": 200 },
                    "context_lines": { "type": "integer", "minimum": 0, "maximum": 10, "description": "Number of surrounding context lines to include before and after each match." },
                    "include": { "type": "string", "description": "Glob pattern for files to include (e.g. '*.rs')." },
                    "exclude": { "type": "string", "description": "Glob pattern for files to exclude (e.g. '*.generated.*')." }
                },
                "required": ["query"]
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
        let root = self.workspace.root.clone();
        Box::pin(async move {
            let args = parse_args::<GrepArgs>(invocation.arguments)?;
            if args.query.is_empty() {
                return Err(ToolError::InvalidArguments("query cannot be empty".into()));
            }

            // Try ripgrep first
            let mut result = try_ripgrep(&root, &args).await?;

            // If ripgrep returned an error (not available, etc), fall back
            if result.as_ref().is_none_or(|v| v.is_empty()) {
                result = Some(grep_fallback(&root, &args)?);
            }

            let matches = result.unwrap_or_default();
            Ok(ToolOutput::new(json!({ "matches": matches })))
        })
    }
}

/// Attempt to use ripgrep (`rg`) for a search. Returns None if rg is not available.
async fn try_ripgrep(root: &Path, args: &GrepArgs) -> Result<Option<Vec<Value>>, ToolError> {
    // Check if rg is available
    let rg_check = tokio::process::Command::new("rg")
        .arg("--version")
        .output()
        .await;
    if rg_check.is_err() {
        return Ok(None);
    }

    let mut cmd = tokio::process::Command::new("rg");
    cmd.arg("--json")
        .arg("--no-heading")
        .arg("--line-number")
        .arg("-s") // case-sensitive
        .current_dir(root);

    // Add context lines
    if let Some(ctx) = args.context_lines {
        cmd.arg("-C").arg(ctx.to_string());
    }

    // Add include/exclude globs
    if let Some(include) = &args.include {
        cmd.arg("--glob").arg(include);
    }
    if let Some(exclude) = &args.exclude {
        cmd.arg("--glob").arg(format!("!{exclude}"));
    }

    // Search path
    let search_path = args.path.as_deref().unwrap_or(".");
    cmd.arg(&args.query).arg(search_path);

    let output = cmd.output().await
        .map_err(|e| ToolError::Execution(format!("rg execution: {e}")))?;

    if !output.status.success() && !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not found") || stderr.contains("no such") {
            return Ok(None);
        }
    }

    let max_matches = args.max_matches.unwrap_or(50).min(200);
    let mut matches: Vec<Value> = Vec::new();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse rg JSON output
    for line in stdout.lines() {
        if matches.len() >= max_matches {
            break;
        }
        if let Ok(parsed) = serde_json::from_str::<Value>(line) {
            let typ = parsed["type"].as_str().unwrap_or("");
            if typ == "match" {
                let data = &parsed["data"];
                let path = data["path"]["text"].as_str().unwrap_or(search_path);
                let line_num = data["line_number"].as_u64().unwrap_or(0);
                let text = data["lines"]["text"].as_str().unwrap_or("").trim();
                matches.push(json!({
                    "path": path,
                    "line": line_num,
                    "text": text,
                }));
            }
        }
    }

    Ok(Some(matches))
}

/// Pure-Rust fallback grep with context_lines, include/exclude support.
fn grep_fallback(root: &Path, args: &GrepArgs) -> Result<Vec<Value>, ToolError> {
    let max_matches = args.max_matches.unwrap_or(50).min(200);
    let context_lines = args.context_lines.unwrap_or(0);

    let include_glob = args.include.as_ref()
        .map(|p| ::glob::Pattern::new(p))
        .and_then(|r| r.ok());
    let exclude_glob = args.exclude.as_ref()
        .map(|p| ::glob::Pattern::new(p))
        .and_then(|r| r.ok());

    let start_path = if let Some(ref path) = args.path {
        root.join(path.trim_start_matches('/'))
    } else {
        root.to_path_buf()
    };

    let mut matches = Vec::new();
    grep_path_with_context(
        root,
        &start_path,
        &args.query,
        max_matches,
        context_lines,
        include_glob.as_ref(),
        exclude_glob.as_ref(),
        &mut matches,
    )?;
    Ok(matches)
}

#[allow(clippy::too_many_arguments)]
fn grep_path_with_context(
    root: &Path,
    path: &Path,
    query: &str,
    max_matches: usize,
    context_lines: usize,
    include_glob: Option<&::glob::Pattern>,
    exclude_glob: Option<&::glob::Pattern>,
    matches: &mut Vec<Value>,
) -> Result<(), ToolError> {
    if matches.len() >= max_matches {
        return Ok(());
    }
    let metadata = fs::metadata(path).map_err(|e| ToolError::Execution(e.to_string()))?;
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|e| ToolError::Execution(e.to_string()))? {
            let entry = entry.map_err(|e| ToolError::Execution(e.to_string()))?;
            grep_path_with_context(
                root, &entry.path(), query, max_matches, context_lines,
                include_glob, exclude_glob, matches,
            )?;
            if matches.len() >= max_matches {
                break;
            }
        }
        return Ok(());
    }

    if metadata.len() > 1_000_000 {
        return Ok(());
    }

    // Apply include/exclude filters
    let rel_path = path.strip_prefix(root).unwrap_or(path);
    if let Some(inc) = include_glob
        && !inc.matches_path(rel_path)
    {
        return Ok(());
    }
    if let Some(exc) = exclude_glob
        && exc.matches_path(rel_path)
    {
        return Ok(());
    }

    let Ok(content) = fs::read_to_string(path) else {
        return Ok(());
    };
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    for (idx, line) in lines.iter().enumerate() {
        if matches.len() >= max_matches {
            break;
        }
        if line.contains(query) {
            let ctx_start = idx.saturating_sub(context_lines);
            let ctx_end = (idx + 1 + context_lines).min(total);

            let mut context: Vec<String> = Vec::new();
            for (ci, line) in lines.iter().enumerate().take(ctx_end).skip(ctx_start) {
                let prefix = if ci == idx { ">" } else { " " };
                context.push(format!("{prefix}{line}"));
            }

            matches.push(json!({
                "path": rel_path.to_string_lossy(),
                "line": idx + 1,
                "text": line,
                "context": context,
            }));
        }
    }
    Ok(())
}

// ── WriteFileTool ───────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct WriteFileTool {
    workspace: WorkspaceTool,
}

impl WriteFileTool {
    fn new(root: PathBuf) -> Self {
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
            fs::write(&path, &args.content)
                .map_err(|error| ToolError::Execution(format!("write failed: {error}")))?;
            Ok(ToolOutput::new(json!({
                "path": args.path,
                "size": args.content.len(),
            })))
        })
    }
}

// ── EditFileTool ────────────────────────────────────────────────────────

/// Standalone API: apply a string replacement to a file.
///
/// Returns the new content on success. Enforces exact-match semantics:
/// - 0 occurrences → error
/// - >1 occurrences + `replace_all=false` → error
pub fn edit_file(
    path: &Path,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Result<String, ToolError> {
    let content = fs::read_to_string(path)
        .map_err(|error| ToolError::Execution(format!("read failed: {error}")))?;

    // Detect line ending style
    let line_ending = detect_line_ending(&content);
    let normalized_new = normalize_line_endings(new_string, line_ending);

    // Count exact matches
    let count = content.matches(old_string).count();
    if count == 0 {
        return Err(ToolError::InvalidArguments(
            "old_string not found in file".into(),
        ));
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

    // Stale-file detection: re-read before writing
    let current = fs::read_to_string(path)
        .map_err(|error| ToolError::Execution(format!("stale check read: {error}")))?;
    if current != content {
        return Err(ToolError::Execution(
            "file changed between read and write — re-read and try again".into(),
        ));
    }

    fs::write(path, &new_content)
        .map_err(|error| ToolError::Execution(format!("write failed: {error}")))?;

    Ok(new_content)
}

fn detect_line_ending(content: &str) -> &str {
    if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn normalize_line_endings(text: &str, line_ending: &str) -> String {
    if line_ending == "\r\n" {
        // Convert LF to CRLF while preserving existing CRLF
        text.lines()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\r\n")
            + if text.ends_with('\n') { line_ending } else { "" }
    } else {
        // Keep as-is (LF)
        text.to_string()
    }
}

#[derive(Clone, Debug)]
struct EditFileTool {
    workspace: WorkspaceTool,
}

impl EditFileTool {
    fn new(root: PathBuf) -> Self {
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

/// A set of command prefixes that are considered reasonably safe and common.
#[allow(dead_code)]
const TRUSTED_COMMAND_PREFIXES: &[&str] = &[
    "cargo ", "cargo test", "cargo build", "cargo check", "cargo fmt",
    "cargo clippy", "cargo doc",
    "git ", "git status", "git diff", "git log", "git show",
    "git branch", "git stash",
    "ls ", "cat ", "head ", "tail ", "echo ", "pwd", "whoami",
    "date", "which ", "type ",
    "mkdir ", "touch ",
];

/// Environment variables that are blocked for security reasons.
const BLOCKED_ENV_VARS: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "LD_DEBUG",
    "SHELL",
    "HOME",
    "PATH",
];

/// Shell command chain separators
const CHAIN_SEPARATORS: &[&str] = &["&&", "||", ";", "|", "`", "$("];

#[derive(Clone, Debug)]
struct ShellTool {
    workspace: WorkspaceTool,
}

#[allow(dead_code)]
impl ShellTool {
    fn new(root: PathBuf) -> Self {
        Self {
            workspace: WorkspaceTool::new(root),
        }
    }

    fn is_trusted_command(command: &str) -> bool {
        let trimmed = command.trim();
        TRUSTED_COMMAND_PREFIXES
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
    }

    /// Analyse a shell command for security concerns.
    /// Returns a list of warning messages (empty = safe to execute).
    fn analyse_command_safety(command: &str) -> Vec<String> {
        let mut warnings: Vec<String> = Vec::new();

        // Check for blocked environment variable assignments
        for var in BLOCKED_ENV_VARS {
            let pattern = format!("{var}=");
            if command.contains(&pattern) {
                warnings.push(format!(
                    "blocked environment variable assignment: {var}"
                ));
            }
        }

        // Check for dangerous patterns in command chains
        let segments = Self::split_command_chain(command);
        for segment in &segments {
            let trimmed = segment.trim();

            // Check for path traversal in shell arguments
            if trimmed.contains("..")
                && (trimmed.contains('/') || trimmed.contains('~'))
                && trimmed.contains("../../") {
                    warnings.push("path traversal detected".into());
                }

            // Warn about background execution
            if trimmed.contains(" &") || trimmed.ends_with('&') {
                warnings.push("background execution may cause unexpected behavior".into());
            }
        }

        // Check for command substitution (can lead to injection)
        if command.contains("$(") || command.contains('`') {
            warnings.push("command substitution detected".into());
        }

        warnings
    }

    /// Split a command into segments at chain separators.
    fn split_command_chain(command: &str) -> Vec<String> {
        let mut segments = vec![command.to_string()];
        for sep in CHAIN_SEPARATORS {
            let mut new_segments = Vec::new();
            for seg in &segments {
                let split: Vec<&str> = seg.split(sep).collect();
                new_segments.extend(split.iter().map(|s| s.to_string()));
            }
            segments = new_segments;
        }
        segments.retain(|s| !s.trim().is_empty());
        segments
    }
}

impl Tool for ShellTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("shell"),
            description: "Execute a shell command in the workspace directory.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute." },
                    "bg": { "type": "boolean", "description": "Run in background. Returns PID immediately." },
                    "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 600, "description": "Timeout in seconds. Default: 120." }
                },
                "required": ["command"]
            }),
            annotations: ToolAnnotations {
                execution: ToolExecution::Sequential,
                mutating: true,
                timeout: Some(std::time::Duration::from_secs(120)),
                capabilities: vec![ToolCapability::ExecutesCode],
                default_approval: ApprovalRequirement::Required,
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        let root = self.workspace.root.clone();
        Box::pin(async move {
            let args = parse_args::<ShellArgs>(invocation.arguments)?;
            let command_str = args.command.trim().to_string();

            if command_str.is_empty() {
                return Err(ToolError::InvalidArguments("command cannot be empty".into()));
            }

            let safety_warnings = Self::analyse_command_safety(&command_str);

            // Background mode — spawn and return immediately
            if args.bg.unwrap_or(false) {
                let child = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(&command_str)
                    .current_dir(&root)
                    .spawn()
                    .map_err(|e| ToolError::Execution(format!("bg spawn failed: {e}")))?;
                let mut res = json!({
                    "bg": true,
                    "pid": child.id(),
                    "command": command_str,
                });
                if !safety_warnings.is_empty() {
                    res["safety_warnings"] = json!(safety_warnings);
                }
                return Ok(ToolOutput::new(res));
            }

            // Foreground execution with tokio, streaming output, and ring buffer
            let mut cmd = tokio::process::Command::new("sh");
            cmd.arg("-c")
                .arg(&command_str)
                .current_dir(&root)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());

            let mut child = cmd
                .spawn()
                .map_err(|e| ToolError::Execution(format!("spawn failed: {e}")))?;

            // Take stdout/stderr before waiting (so we can still read on timeout)
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            // Spawn tasks to read output asynchronously
            let stdout_task = tokio::spawn(async move {
                let mut buf = Vec::new();
                if let Some(mut r) = stdout {
                    let _ = r.read_to_end(&mut buf).await;
                }
                buf
            });
            let stderr_task = tokio::spawn(async move {
                let mut buf = Vec::new();
                if let Some(mut r) = stderr {
                    let _ = r.read_to_end(&mut buf).await;
                }
                buf
            });

            let timeout_dur = args
                .timeout_secs
                .map(std::time::Duration::from_secs)
                .unwrap_or(std::time::Duration::from_secs(120));

            let result = tokio::time::timeout(timeout_dur, child.wait()).await;

            let (status, raw_stdout, raw_stderr) = match result {
                Ok(Ok(status)) => {
                    // Child exited — pipes are closed, read tasks will finish
                    let out = stdout_task.await.unwrap_or_default();
                    let err = stderr_task.await.unwrap_or_default();
                    (Some(status), out, err)
                }
                Ok(Err(e)) => return Err(ToolError::Execution(format!("command failed: {e}"))),
                Err(_) => {
                    // Timeout — kill the child process
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    let out = stdout_task.await.unwrap_or_default();
                    let err = stderr_task.await.unwrap_or_default();
                    let (stdout_txt, stderr_txt, spill_path) =
                        build_ring_buffer(&out, &err, 64_000, 8_000);
                    let mut res = json!({
                        "stdout": stdout_txt,
                        "stderr": stderr_txt,
                        "exit_code": null,
                        "success": false,
                    });
                    if let Some(spill) = spill_path {
                        res["spill_path"] = json!(spill.to_string_lossy());
                    }
                    if !safety_warnings.is_empty() {
                        res["safety_warnings"] = json!(safety_warnings);
                    }
                    return Ok(ToolOutput::new(res));
                }
            };

            let exit_code = status.as_ref().and_then(|s| s.code());
            let success = status.is_some_and(|s| s.success());

            let (stdout_txt, stderr_txt, spill_path) =
                build_ring_buffer(&raw_stdout, &raw_stderr, 64_000, 8_000);

            let mut res = json!({
                "stdout": stdout_txt,
                "stderr": stderr_txt,
                "exit_code": exit_code,
                "success": success,
            });

            if let Some(spill) = spill_path {
                res["spill_path"] = json!(spill.to_string_lossy());
            }
            if !safety_warnings.is_empty() {
                res["safety_warnings"] = json!(safety_warnings);
            }
            Ok(ToolOutput::new(res))
        })
    }
}

/// Apply a ring buffer to command output: keep up to `max_stdout`/`max_stderr`
/// bytes, spill the excess to a temp file.
fn build_ring_buffer(
    stdout: &[u8],
    stderr: &[u8],
    max_stdout: usize,
    max_stderr: usize,
) -> (String, String, Option<PathBuf>) {
    let stdout_str = String::from_utf8_lossy(stdout);
    let stderr_str = String::from_utf8_lossy(stderr);

    let mut spill_path = None;

    let stdout_result = if stdout_str.len() > max_stdout {
        let spill_dir = std::env::temp_dir().join("joker-shell-spill");
        let _ = fs::create_dir_all(&spill_dir);
        let path = spill_dir.join(format!("stdout-{}", std::process::id()));
        let _ = fs::write(&path, stdout_str.as_ref());

        let truncated =
            truncate_at_char_boundary(stdout_str.as_ref(), max_stdout).to_string();
        spill_path = Some(path);
        format!(
            "{}\n... [output truncated, {} bytes total]",
            truncated,
            stdout_str.len()
        )
    } else {
        stdout_str.to_string()
    };

    let stderr_result = if stderr_str.len() > max_stderr {
        let t = truncate_at_char_boundary(stderr_str.as_ref(), max_stderr).to_string();
        format!(
            "{}\n... [stderr truncated, {} bytes total]",
            t,
            stderr_str.len()
        )
    } else {
        stderr_str.to_string()
    };

    (stdout_result, stderr_result, spill_path)
}

// ── ApplyPatchTool ─────────────────────────────────────────────────────

/// A simple unified-diff patcher for workspace files.
/// Expects a standard unified diff patch string and applies it to the target file.
#[derive(Clone, Debug)]
struct ApplyPatchTool {
    workspace: WorkspaceTool,
}

impl ApplyPatchTool {
    fn new(root: PathBuf) -> Self {
        Self {
            workspace: WorkspaceTool::new(root),
        }
    }

    /// Parse and apply a unified diff patch to the given content.
    /// Returns the patched content on success.
    fn apply_patch(content: &str, patch_str: &str) -> Result<String, ToolError> {
        let mut result = content.to_string();
        let mut patch_pos = 0usize;
        let _content_lines: Vec<&str> = content.lines().collect();

        // Parse hunks from the patch
        let patch_lines: Vec<&str> = patch_str.lines().collect();
        while patch_pos < patch_lines.len() {
            let line = patch_lines[patch_pos];

            // Skip header lines (---/+++)
            if line.starts_with("--- ") || line.starts_with("+++ ") || line.is_empty() {
                patch_pos += 1;
                continue;
            }

            // Look for hunk header: @@ -start,count +start,count @@
            if let Some(header) = line.strip_prefix("@@ ") {
                if let Some(hunk_end) = header.find(" @@") {
                    let hunk_header = &header[..hunk_end];
                    let parts: Vec<&str> = hunk_header.split_whitespace().collect();
                    if parts.len() < 2 {
                        return Err(ToolError::InvalidArguments(
                            format!("invalid hunk header: {line}"),
                        ));
                    }
                    // Parse the -hunk start line (remove leading '-')
                    let old_start_str = parts[0].strip_prefix('-').unwrap_or(parts[0]);
                    // Parse old start line number, stripping comma-count if present
                    let old_start: usize = old_start_str
                        .split(',')
                        .next()
                        .unwrap_or("1")
                        .parse()
                        .map_err(|_| {
                            ToolError::InvalidArguments(format!(
                                "invalid line number in hunk: {line}"
                            ))
                        })?;

                    // Collect hunk body
                    patch_pos += 1;
                    let mut hunk_removals: Vec<String> = Vec::new();
                    let mut hunk_additions: Vec<String> = Vec::new();
                    let mut has_context = false;

                    while patch_pos < patch_lines.len() {
                        let hunk_line = patch_lines[patch_pos];
                        if hunk_line.starts_with("@@ ") {
                            break; // next hunk
                        }
                        if let Some(stripped) = hunk_line.strip_prefix('-') {
                            hunk_removals.push(stripped.to_string());
                        } else if let Some(stripped) = hunk_line.strip_prefix('+') {
                            hunk_additions.push(stripped.to_string());
                        } else {
                            // Context line (space-prefixed or empty)
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

                    // Find and apply the hunk in the content
                    let content_line_idx = old_start.saturating_sub(1);
                    let current_lines: Vec<&str> = result.lines().collect();

                    if content_line_idx >= current_lines.len() {
                        // If file doesn't have this line yet, append additions
                        if !hunk_additions.is_empty() {
                            result.push('\n');
                            result.push_str(&hunk_additions.join("\n"));
                        }
                        continue;
                    }

                    // Try to match removals starting at content_line_idx
                    let removal_text = hunk_removals.join("\n");
                    let addition_text = hunk_additions.join("\n");

                    // Build the content area to search in
                    let line_count = hunk_removals.len();
                    let end = std::cmp::min(content_line_idx + line_count, current_lines.len());
                    let search_section = current_lines[content_line_idx..end].join("\n");

                    if search_section == removal_text || (!has_context && removal_text.is_empty()) {
                        // Match found — replace
                        let before: Vec<&str> = current_lines[..content_line_idx].to_vec();
                        let after: Vec<&str> = if content_line_idx + line_count < current_lines.len()
                        {
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
                    }
                    // If no match found, try searching the entire content
                    else if let Some(pos) = result.find(&removal_text)
                        && !removal_text.is_empty() {
                            result.replace_range(pos..pos + removal_text.len(), &addition_text);
                        }
                    // If nothing matches, just skip this hunk
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
            let args = parse_args::<PatchArgs>(invocation.arguments)?;
            let path = self.workspace.resolve_write(&args.path)?;

            // Read the existing file (if it exists)
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

// ── Utility functions ───────────────────────────────────────────────────

pub(crate) fn parse_args<T>(value: Value) -> Result<T, ToolError>
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

// ── Argument types ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PathArgs {
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReadFileArgs {
    path: String,
    max_bytes: Option<usize>,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct GrepArgs {
    query: String,
    path: Option<String>,
    max_matches: Option<usize>,
    context_lines: Option<usize>,
    include: Option<String>,
    exclude: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct EditFileArgs {
    path: String,
    old_string: String,
    new_string: String,
    replace_all: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ShellArgs {
    command: String,
    bg: Option<bool>,
    timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct PatchArgs {
    path: String,
    patch: String,
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
}
