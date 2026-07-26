//! Standard tool registries for Joker — readonly, writeable, and full sets.
//!
//! This crate builds [`ToolRegistry`](joker::ToolRegistry) instances with the
//! built-in file-system tools (read, write, edit, grep, glob, shell, etc.)
//! and optional extensions like web search and memory.

#![forbid(unsafe_code)]
#![deny(unreachable_pub)]
#![warn(missing_docs)]

use std::path::PathBuf;
use std::sync::Arc;

/// Workspace resolution utilities.
pub mod workspace;
mod apply_patch;
mod edit;
mod fetch_url;
/// File glob matching tool.
pub mod glob;
mod grep;
mod list_files;
/// Persistent key-value memory for agent state.
pub mod memory;
mod read_file;
mod shell;
/// TODO task management tool.
pub mod todo;
mod write_file;
/// Web search via DuckDuckGo.
pub mod web_search;

use joker::ToolRegistry;

#[doc(inline)]
pub use edit::edit_file;
#[doc(inline)]
pub use todo::TodoItem;

use apply_patch::ApplyPatchTool;
use edit::EditFileTool;
use fetch_url::FetchUrlTool;
use glob::GlobTool;
use grep::GrepTool;
use list_files::ListFilesTool;
use joker::WebSearch;
use memory::{MemoryReadTool, MemoryWriteTool};
use read_file::ReadFileTool;
use shell::ShellTool;
use todo::TodoWriteTool;
use web_search::{DuckDuckGoSearch, WebSearchTool};
use write_file::WriteFileTool;

/// Errors that can occur when building or using a tool registry.
#[derive(Debug, thiserror::Error)]
pub enum ToolsError {
    /// A tool could not be registered in the registry.
    #[error("tool registry error: {0}")]
    Registry(#[from] joker::ToolError),
    /// A workspace path could not be resolved.
    #[error("workspace path error: {0}")]
    Workspace(std::io::Error),
    /// A general execution error.
    #[error("execution error: {0}")]
    ExecutionError(String),
}

/// Build a readonly [`ToolRegistry`](joker::ToolRegistry) wrapped in an [`Arc`].
pub fn readonly_tools(workspace: impl Into<PathBuf>) -> Result<Arc<ToolRegistry>, ToolsError> {
    Ok(Arc::new(readonly_tool_registry(workspace)?))
}

/// Build a readonly [`ToolRegistry`](joker::ToolRegistry) with file read/search tools.
///
/// Includes [`ListFilesTool`], [`ReadFileTool`], [`GrepTool`], and [`GlobTool`].
pub fn readonly_tool_registry(workspace: impl Into<PathBuf>) -> Result<ToolRegistry, ToolsError> {
    let workspace = workspace.into();
    let mut registry = ToolRegistry::new();
    registry.insert(ListFilesTool::new(workspace.clone()))?;
    registry.insert(ReadFileTool::new(workspace.clone()))?;
    registry.insert(GrepTool::new(workspace.clone()))?;
    registry.insert(GlobTool::new(workspace.clone()))?;
    Ok(registry)
}

/// Build a writeable [`ToolRegistry`](joker::ToolRegistry) wrapped in an [`Arc`].
///
/// Extends the readonly set with write/edit/shell/fetch/patch tools.
pub fn writeable_tools(workspace: impl Into<PathBuf>) -> Result<Arc<ToolRegistry>, ToolsError> {
    Ok(Arc::new(writeable_tool_registry(workspace)?))
}

/// Build a writeable [`ToolRegistry`](joker::ToolRegistry) extending the readonly set.
///
/// Adds [`WriteFileTool`], [`EditFileTool`], [`ShellTool`], [`ApplyPatchTool`],
/// [`FetchUrlTool`], and [`TodoWriteTool`].
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

/// Build a full [`ToolRegistry`](joker::ToolRegistry) (all tools) wrapped in an [`Arc`].
pub fn all_tools(workspace: impl Into<PathBuf>) -> Result<Arc<ToolRegistry>, ToolsError> {
    Ok(Arc::new(all_tool_registry(workspace)?))
}

/// Build a full [`ToolRegistry`](joker::ToolRegistry) with all available tools.
///
/// Extends the writeable set with web search (if the backend is available)
/// and memory read/write tools.
pub fn all_tool_registry(
    workspace: impl Into<PathBuf>,
) -> Result<ToolRegistry, ToolsError> {
    let workspace = workspace.into();
    let mut registry = writeable_tool_registry(workspace.clone())?;

    match DuckDuckGoSearch::new() {
        Ok(backend) => {
            registry.insert(WebSearchTool::new(
                std::sync::Arc::new(backend) as Arc<dyn WebSearch>
            ))?;
        }
        Err(e) => {
            eprintln!("warning: failed to initialize web search: {e}");
        }
    }

    registry.insert(MemoryReadTool::new(workspace.clone()))?;
    registry.insert(MemoryWriteTool::new(workspace.clone()))?;

    Ok(registry)
}
