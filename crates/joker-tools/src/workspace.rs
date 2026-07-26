use std::fs;
use std::path::PathBuf;

use joker::ToolError;
use serde::Deserialize;

/// Workspace-scoped path resolver that prevents path traversal attacks.
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

    /// Resolve a path for writing — the target must be within the workspace.
    /// Unlike `resolve_read`, intermediate directories need not exist yet.
    pub(crate) fn resolve_write(&self, path: &str) -> Result<PathBuf, ToolError> {
        let root = fs::canonicalize(&self.root)
            .map_err(|error| ToolError::Execution(format!("workspace does not exist: {error}")))?;
        let candidate = root.join(path.trim_start_matches('/'));

        // Walk up from the candidate until we find an existing ancestor.
        // Canonicalize that to verify it's within the workspace, then re-attach
        // the trailing components that didn't exist yet.
        let mut components: Vec<&std::path::Path> = Vec::new();
        let mut ancestor = candidate.as_path();

        loop {
            if ancestor.exists() {
                let resolved = fs::canonicalize(ancestor)
                    .map_err(|error| ToolError::Execution(format!(
                        "failed to resolve path: {}: {error}", ancestor.display()
                    )))?;
                if !resolved.starts_with(&root) {
                    return Err(ToolError::InvalidArguments(format!(
                        "path escapes workspace: {path}"
                    )));
                }
                let result = components
                    .into_iter()
                    .rev()
                    .fold(resolved, |acc, c| acc.join(c));
                return Ok(result);
            }
            match ancestor.parent() {
                Some(parent) => {
                    let last = ancestor
                        .file_name()
                        .ok_or_else(|| ToolError::InvalidArguments(format!("invalid path: {path}")))?;
                    components.push(std::path::Path::new(last));
                    ancestor = parent;
                }
                None => break,
            }
        }

        // Nothing existed at all — fall through to root check
        let root_parent = root.parent().unwrap_or(&root);
        if !candidate.starts_with(root_parent) {
            return Err(ToolError::InvalidArguments(format!(
                "path escapes workspace: {path}"
            )));
        }
        Ok(candidate)
    }
}

pub(crate) fn parse_args<T>(value: serde_json::Value) -> Result<T, ToolError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(value).map_err(|error| ToolError::InvalidArguments(error.to_string()))
}

pub(crate) fn truncate_at_char_boundary(text: &str, max_bytes: usize) -> &str {
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

pub(crate) fn detect_line_ending(content: &str) -> &str {
    if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

#[allow(dead_code)]
pub(crate) fn normalize_line_endings(text: &str, line_ending: &str) -> String {
    if line_ending == "\r\n" {
        text.lines()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\r\n")
            + if text.ends_with('\n') { line_ending } else { "" }
    } else {
        text.to_string()
    }
}
