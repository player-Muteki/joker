use std::fs;
use std::path::{Component, Path, PathBuf};

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

    fn workspace_relative_path(path: &str) -> Result<PathBuf, ToolError> {
        let path = path.trim();
        if path.is_empty() {
            return Err(ToolError::InvalidArguments("path cannot be empty".into()));
        }

        let mut normalized = PathBuf::new();
        for component in Path::new(path).components() {
            match component {
                Component::Prefix(_) | Component::RootDir => {
                    return Err(ToolError::InvalidArguments(format!(
                        "absolute paths are not allowed: {path}"
                    )));
                }
                Component::CurDir => {}
                Component::Normal(part) => normalized.push(part),
                Component::ParentDir => {
                    if !normalized.pop() {
                        return Err(ToolError::InvalidArguments(format!(
                            "path escapes workspace: {path}"
                        )));
                    }
                }
            }
        }

        if normalized.as_os_str().is_empty() {
            return Err(ToolError::InvalidArguments(format!("invalid path: {path}")));
        }
        Ok(normalized)
    }

    /// Resolve a path for reading — the path must exist and be within the workspace.
    pub(crate) fn resolve_read(&self, path: &str) -> Result<PathBuf, ToolError> {
        let root = fs::canonicalize(&self.root)
            .map_err(|error| ToolError::Execution(format!("workspace does not exist: {error}")))?;
        let candidate = root.join(Self::workspace_relative_path(path)?);
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
        let candidate = root.join(Self::workspace_relative_path(path)?);

        // Walk up from the candidate until we find an existing ancestor.
        // Canonicalize that to verify it's within the workspace, then re-attach
        // the trailing components that didn't exist yet.
        let mut components: Vec<&std::path::Path> = Vec::new();
        let mut ancestor = candidate.as_path();

        loop {
            if ancestor.exists() {
                let resolved = fs::canonicalize(ancestor).map_err(|error| {
                    ToolError::Execution(format!(
                        "failed to resolve path: {}: {error}",
                        ancestor.display()
                    ))
                })?;
                if !resolved.starts_with(&root) {
                    return Err(ToolError::InvalidArguments(format!(
                        "path escapes workspace: {path}"
                    )));
                }
                let result = components
                    .into_iter()
                    .rev()
                    .fold(resolved, |acc, c| acc.join(c));
                if !result.starts_with(&root) {
                    return Err(ToolError::InvalidArguments(format!(
                        "path escapes workspace: {path}"
                    )));
                }
                return Ok(result);
            }
            match ancestor.parent() {
                Some(parent) => {
                    let last = ancestor.file_name().ok_or_else(|| {
                        ToolError::InvalidArguments(format!("invalid path: {path}"))
                    })?;
                    components.push(std::path::Path::new(last));
                    ancestor = parent;
                }
                None => break,
            }
        }

        // Nothing existed at all — fall through to root check
        if !candidate.starts_with(&root) {
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
            + if text.ends_with('\n') {
                line_ending
            } else {
                ""
            }
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("joker-workspace-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn resolve_write_rejects_paths_that_escape_workspace() {
        let dir = TempDir::new("escape");
        let tool = WorkspaceTool::new(dir.path.clone());

        assert!(tool.resolve_write("../outside.txt").is_err());
        assert!(tool.resolve_write("nested/../../outside.txt").is_err());
    }

    #[test]
    fn resolve_write_normalizes_safe_parent_components() {
        let dir = TempDir::new("normalize");
        let tool = WorkspaceTool::new(dir.path.clone());

        let resolved = tool.resolve_write("nested/../inside.txt").unwrap();
        assert_eq!(
            resolved,
            fs::canonicalize(&dir.path).unwrap().join("inside.txt")
        );
    }
}
