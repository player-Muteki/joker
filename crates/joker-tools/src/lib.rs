#![forbid(unsafe_code)]
#![deny(unreachable_pub)]
#![warn(missing_docs)]

use std::path::PathBuf;
use std::sync::Arc;

pub mod workspace;
mod apply_patch;
mod edit;
mod fetch_url;
pub mod glob;
mod grep;
mod list_files;
pub mod memory;
mod read_file;
mod shell;
pub mod todo;
mod write_file;
pub mod web_search;

use joker::ToolRegistry;

pub use edit::edit_file;
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

#[derive(Debug, thiserror::Error)]
pub enum ToolsError {
    #[error("tool registry error: {0}")]
    Registry(#[from] joker::ToolError),
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

pub fn all_tools(workspace: impl Into<PathBuf>) -> Result<Arc<ToolRegistry>, ToolsError> {
    Ok(Arc::new(all_tool_registry(workspace)?))
}

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
