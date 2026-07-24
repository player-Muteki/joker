#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

use std::{
    fs,
    path::{Path, PathBuf},
    process,
    sync::Arc,
};

use joker::{
    Tool, ToolAnnotations, ToolDefinition, ToolError, ToolExecution, ToolFuture, ToolInvocation,
    ToolName, ToolOutput, ToolRegistry,
};
use serde::{Deserialize, Serialize};
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
    registry.insert(GrepTool::new(workspace))?;
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

    // Web search tool — uses Brave Search API from env if available
    let search_provider: Option<std::sync::Arc<dyn WebSearch>> = BraveSearchProvider::from_env()
        .map(|p| std::sync::Arc::new(p) as std::sync::Arc<dyn WebSearch>);
    registry.insert(WebSearchTool::new(search_provider))?;

    // URL fetch tool
    registry.insert(FetchUrlTool)?;

    Ok(registry)
}

// ── Workspace path helper ───────────────────────────────────────────────

#[derive(Clone, Debug)]
struct WorkspaceTool {
    root: PathBuf,
}

impl WorkspaceTool {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Resolve a path for reading — the path must exist and be within the workspace.
    fn resolve_read(&self, path: &str) -> Result<PathBuf, ToolError> {
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
    fn resolve_write(&self, path: &str) -> Result<PathBuf, ToolError> {
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
            let path = self.workspace.resolve_read(&args.path)?;
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
                .resolve_read(args.path.as_deref().unwrap_or("."))?;
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
            description: "Replace the first occurrence of a string in a file with new content.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative file path." },
                    "old_string": { "type": "string", "description": "The exact text to replace." },
                    "new_string": { "type": "string", "description": "The replacement text." }
                },
                "required": ["path", "old_string", "new_string"]
            }),
            annotations: ToolAnnotations {
                execution: ToolExecution::Sequential,
                mutating: true,
                timeout: None,
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async move {
            let args = parse_args::<EditFileArgs>(invocation.arguments)?;
            let path = self.workspace.resolve_read(&args.path)?;
            let content = fs::read_to_string(&path)
                .map_err(|error| ToolError::Execution(format!("read failed: {error}")))?;

            if !content.contains(&args.old_string) {
                return Err(ToolError::InvalidArguments(
                    "old_string not found in file".into(),
                ));
            }

            // Replace only the first occurrence
            let new_content = content.replacen(&args.old_string, &args.new_string, 1);
            fs::write(&path, &new_content)
                .map_err(|error| ToolError::Execution(format!("write failed: {error}")))?;

            Ok(ToolOutput::new(json!({
                "path": args.path,
                "replaced": true,
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
            {
                if trimmed.contains("../../") {
                    warnings.push("path traversal detected".into());
                }
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
                    "command": { "type": "string", "description": "Shell command to execute." }
                },
                "required": ["command"]
            }),
            annotations: ToolAnnotations {
                execution: ToolExecution::Sequential,
                mutating: true,
                timeout: Some(std::time::Duration::from_secs(120)),
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async move {
            let args = parse_args::<ShellArgs>(invocation.arguments)?;
            let command_str = args.command.trim().to_string();

            if command_str.is_empty() {
                return Err(ToolError::InvalidArguments("command cannot be empty".into()));
            }

            // Run safety analysis
            let safety_warnings = Self::analyse_command_safety(&command_str);

            // Execute the command via shell
            let output = process::Command::new("sh")
                .arg("-c")
                .arg(&command_str)
                .current_dir(&self.workspace.root)
                .output()
                .map_err(|error| ToolError::Execution(format!("command failed: {error}")))?;

            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let exit_code = output.status.code();

            let mut result = json!({
                "stdout": truncate_large_output(&stdout, 32_000),
                "stderr": truncate_large_output(&stderr, 8_000),
                "exit_code": exit_code,
                "success": output.status.success(),
            });

            if !safety_warnings.is_empty() {
                result["safety_warnings"] = json!(safety_warnings);
            }

            Ok(ToolOutput::new(result))
        })
    }
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
                        if hunk_line.starts_with('-') {
                            hunk_removals.push(hunk_line[1..].to_string());
                        } else if hunk_line.starts_with('+') {
                            hunk_additions.push(hunk_line[1..].to_string());
                        } else {
                            // Context line (space-prefixed or empty)
                            let ctx = if hunk_line.starts_with(' ') {
                                &hunk_line[1..]
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
                            result.push_str("\n");
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
                    else if let Some(pos) = result.find(&removal_text) {
                        if !removal_text.is_empty() {
                            result.replace_range(pos..pos + removal_text.len(), &addition_text);
                        }
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

fn truncate_large_output(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut result = truncate_at_char_boundary(text, max_bytes).to_string();
    result.push_str(&format!("\n... [truncated {} bytes]", text.len() - max_bytes));
    result
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
}

#[derive(Debug, Deserialize)]
struct GrepArgs {
    query: String,
    path: Option<String>,
    max_matches: Option<usize>,
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
}

#[derive(Debug, Deserialize)]
struct ShellArgs {
    command: String,
}

#[derive(Debug, Deserialize)]
struct PatchArgs {
    path: String,
    patch: String,
}

// ── WebSearch trait and network tools ───────────────────────────────────

use std::fmt;

/// Standardized search result item.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchResultItem {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Standardized search response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchResult {
    pub items: Vec<SearchResultItem>,
    pub total_estimate: Option<u64>,
}

/// Trait for web search providers.
#[async_trait::async_trait]
pub trait WebSearch: Send + Sync {
    /// Search the web for the given query.
    /// Returns a structured SearchResult.
    async fn search(&self, query: &str, count: usize) -> Result<SearchResult, ToolsError>;
}

/// A simple built-in search provider that uses the Brave Search API.
/// Requires the `BRAVE_SEARCH_API_KEY` environment variable to be set.
pub struct BraveSearchProvider {
    api_key: String,
    client: reqwest::Client,
}

impl BraveSearchProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .user_agent("Joker/0.1")
                .build()
                .unwrap_or_default(),
        }
    }

    /// Create from environment variable, returning None if not set.
    pub fn from_env() -> Option<Self> {
        std::env::var("BRAVE_SEARCH_API_KEY").ok().map(Self::new)
    }
}

#[async_trait::async_trait]
impl WebSearch for BraveSearchProvider {
    async fn search(&self, query: &str, count: usize) -> Result<SearchResult, ToolsError> {
        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            urlencoding(query),
            count.min(10)
        );

        let response = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", &self.api_key)
            .send()
            .await
            .map_err(|e| ToolsError::ExecutionError(format!("search request failed: {e}")))?;

        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ToolsError::ExecutionError(format!("search response parse failed: {e}")))?;

        if !status.is_success() {
            return Err(ToolsError::ExecutionError(format!(
                "search API returned {status}: {body}"
            )));
        }

        let mut items = Vec::new();
        if let Some(web) = body.get("web") {
            if let Some(results) = web.get("results").and_then(|r| r.as_array()) {
                for result in results {
                    items.push(SearchResultItem {
                        title: result
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        url: result
                            .get("url")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        snippet: result
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    });
                }
            }
        }

        let total = body
            .get("web")
            .and_then(|w| w.get("total_results"))
            .and_then(|v| v.as_u64());

        Ok(SearchResult {
            items,
            total_estimate: total,
        })
    }
}

fn urlencoding(input: &str) -> String {
    let mut result = String::new();
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push_str("%20"),
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

/// A web search tool that delegates to a [`WebSearch`] provider.
#[derive(Clone)]
pub struct WebSearchTool {
    provider: Option<std::sync::Arc<dyn WebSearch>>,
}

impl WebSearchTool {
    #[must_use]
    pub fn new(provider: Option<std::sync::Arc<dyn WebSearch>>) -> Self {
        Self { provider }
    }

    fn fmt(&self) -> &'static str {
        "WebSearchTool"
    }
}

impl fmt::Debug for WebSearchTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebSearchTool").finish()
    }
}

impl Tool for WebSearchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("web_search"),
            description: "Search the web for information. Returns a list of relevant URLs and summaries.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query." },
                    "count": { "type": "integer", "minimum": 1, "maximum": 10, "default": 5 }
                },
                "required": ["query"]
            }),
            annotations: ToolAnnotations {
                execution: ToolExecution::Sequential,
                mutating: false,
                timeout: Some(std::time::Duration::from_secs(20)),
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        let provider = self.provider.clone();
        Box::pin(async move {
            let args = parse_args::<WebSearchArgs>(invocation.arguments)?;
            let count = args.count.unwrap_or(5).min(10);

            let Some(ref provider) = provider else {
                return Ok(ToolOutput::new(json!({
                    "error": "no search provider configured. Set BRAVE_SEARCH_API_KEY environment variable.",
                    "items": [],
                })));
            };

            match provider.search(&args.query, count).await {
                Ok(result) => {
                    let items: Vec<serde_json::Value> = result
                        .items
                        .into_iter()
                        .map(|item| {
                            json!({
                                "title": item.title,
                                "url": item.url,
                                "snippet": item.snippet,
                            })
                        })
                        .collect();
                    Ok(ToolOutput::new(json!({
                        "items": items,
                        "total_estimate": result.total_estimate,
                    })))
                }
                Err(e) => Ok(ToolOutput::new(json!({
                    "error": e.to_string(),
                    "items": [],
                }))),
            }
        })
    }
}

/// A tool that fetches a URL and returns its content as text.
#[derive(Clone, Debug)]
pub struct FetchUrlTool;

impl Tool for FetchUrlTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("fetch_url"),
            description: "Fetch a URL and return its text content.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The URL to fetch." },
                    "max_bytes": { "type": "integer", "minimum": 1, "maximum": 200000, "default": 50000 }
                },
                "required": ["url"]
            }),
            annotations: ToolAnnotations {
                execution: ToolExecution::Sequential,
                mutating: false,
                timeout: Some(std::time::Duration::from_secs(30)),
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async move {
            let args = parse_args::<FetchUrlArgs>(invocation.arguments)?;

            // Basic URL validation
            if !args.url.starts_with("http://") && !args.url.starts_with("https://") {
                return Err(ToolError::InvalidArguments(
                    "URL must start with http:// or https://".into(),
                ));
            }

            let max_bytes = args.max_bytes.unwrap_or(50_000).min(200_000);
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(25))
                .user_agent("Joker/0.1 (research agent)")
                .build()
                .map_err(|e| ToolError::Execution(format!("failed to create HTTP client: {e}")))?;

            let response = client
                .get(&args.url)
                .send()
                .await
                .map_err(|e| ToolError::Execution(format!("request failed: {e}")))?;

            let status = response.status();
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();

            let body = response
                .bytes()
                .await
                .map_err(|e| ToolError::Execution(format!("read failed: {e}")))?;

            let size = body.len();
            let text = if content_type.contains("application/json") || content_type.contains("text/")
            {
                String::from_utf8_lossy(&body).to_string()
            } else {
                format!("[binary content: {size} bytes, content-type: {content_type}]")
            };

            let truncated = text.len() > max_bytes;
            let content = if truncated {
                truncate_at_char_boundary(&text, max_bytes).to_string()
            } else {
                text
            };

            Ok(ToolOutput::new(json!({
                "url": args.url,
                "status": status.as_u16(),
                "content_type": content_type,
                "size": size,
                "content": content,
                "truncated": truncated,
            })))
        })
    }
}

#[derive(Debug, Deserialize)]
struct WebSearchArgs {
    query: String,
    count: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct FetchUrlArgs {
    url: String,
    max_bytes: Option<usize>,
}

// ── Tests ───────────────────────────────────────────────────────────────

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

    #[tokio::test]
    async fn write_file_creates_content() {
        let root = std::env::temp_dir().join(format!("joker-tools-write-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let registry = writeable_tools(&root).unwrap();

        let result = registry
            .call(ToolInvocation {
                call_id: "1".into(),
                name: ToolName::new("write_file"),
                arguments: json!({"path":"test.txt", "content":"hello world"}),
            })
            .await;

        assert!(!result.is_error, "write failed: {:?}", result.output);
        assert_eq!(result.output["size"], 11);

        let content = fs::read_to_string(root.join("test.txt")).unwrap();
        assert_eq!(content, "hello world");

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn edit_file_replaces_content() {
        let root = std::env::temp_dir().join(format!("joker-tools-edit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("edit.txt"), "hello world").unwrap();
        let registry = writeable_tools(&root).unwrap();

        let result = registry
            .call(ToolInvocation {
                call_id: "1".into(),
                name: ToolName::new("edit_file"),
                arguments: json!({"path":"edit.txt", "old_string":"world", "new_string":"there"}),
            })
            .await;

        assert!(!result.is_error, "edit failed: {:?}", result.output);

        let content = fs::read_to_string(root.join("edit.txt")).unwrap();
        assert_eq!(content, "hello there");

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn edit_file_errors_if_old_string_missing() {
        let root = std::env::temp_dir().join(format!("joker-tools-edit2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("edit.txt"), "hello world").unwrap();
        let registry = writeable_tools(&root).unwrap();

        let result = registry
            .call(ToolInvocation {
                call_id: "1".into(),
                name: ToolName::new("edit_file"),
                arguments: json!({"path":"edit.txt", "old_string":"missing", "new_string":"there"}),
            })
            .await;

        assert!(result.is_error);
        assert!(result.output.as_str().unwrap().contains("not found"));

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn write_file_rejects_escape() {
        let root = std::env::temp_dir().join(format!("joker-tools-escape-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let registry = writeable_tools(&root).unwrap();

        let result = registry
            .call(ToolInvocation {
                call_id: "1".into(),
                name: ToolName::new("write_file"),
                arguments: json!({"path":"../../etc/passwd", "content":"evil"}),
            })
            .await;

        assert!(result.is_error, "should reject path escape");

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn shell_executes_command() {
        let root = std::env::temp_dir().join(format!("joker-tools-shell-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let registry = writeable_tools(&root).unwrap();

        let result = registry
            .call(ToolInvocation {
                call_id: "1".into(),
                name: ToolName::new("shell"),
                arguments: json!({"command":"echo hello"}),
            })
            .await;

        assert!(!result.is_error, "shell failed: {:?}", result.output);
        assert!(result.output["stdout"].as_str().unwrap().contains("hello"));

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn apply_patch_modifies_file() {
        let root =
            std::env::temp_dir().join(format!("joker-tools-patch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("example.rs"),
            "fn hello() {\n    println!(\"old\");\n}\n",
        )
        .unwrap();
        let registry = writeable_tools(&root).unwrap();

        let patch = [
            "@@ -1,3 +1,3 @@",
            " fn hello() {",
            "-    println!(\"old\");",
            "+    println!(\"new\");",
            " }",
        ]
        .join("\n");

        let result = registry
            .call(ToolInvocation {
                call_id: "1".into(),
                name: ToolName::new("apply_patch"),
                arguments: json!({"path":"example.rs", "patch": patch}),
            })
            .await;

        assert!(!result.is_error, "patch failed: {:?}", result.output);

        let content = fs::read_to_string(root.join("example.rs")).unwrap();
        assert!(content.contains("println!(\"new\")"));
        assert!(!content.contains("println!(\"old\")"));

        let _ = fs::remove_dir_all(&root);
    }
}
