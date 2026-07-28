use std::{collections::HashMap, sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::time;

use crate::{ToolResult, error::BoxFutureResult};

/// Result type returned by [`Tool::call`].
pub type ToolFuture<'a> = BoxFutureResult<'a, ToolOutput, ToolError>;

/// Core abstraction for a tool that can be called by an LLM.
pub trait Tool: Send + Sync {
    /// Returns the tool's metadata (name, description, schema, capabilities).
    fn definition(&self) -> ToolDefinition;
    /// Invokes the tool with the given arguments and returns the output.
    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_>;
}

/// A named identifier for a [`Tool`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolName(String);

impl ToolName {
    /// Creates a new [`ToolName`] from any string-like value.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// Returns the underlying name as a `&str`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ToolName {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for ToolName {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl std::fmt::Display for ToolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Metadata describing a [`Tool`]: its name, description, JSON input schema, and
/// runtime annotations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// The tool's unique name.
    pub name: ToolName,
    /// A human-readable description of what the tool does.
    pub description: String,
    /// JSON Schema describing the expected invocation arguments.
    pub input_schema: Value,
    /// Runtime annotations (execution mode, mutability, capabilities, etc.).
    pub annotations: ToolAnnotations,
}

/// Annotations that control a [`Tool`]'s runtime behavior.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolAnnotations {
    /// Whether the tool must run sequentially or is safe to run in parallel.
    pub execution: ToolExecution,
    /// Whether the tool mutates state (e.g., writes files).
    pub mutating: bool,
    /// Optional maximum duration before the tool invocation is aborted.
    pub timeout: Option<Duration>,
    /// The capabilities this tool requires (read-only, writes files, etc.).
    pub capabilities: Vec<ToolCapability>,
    /// The default approval strategy for invocations of this tool.
    pub default_approval: ApprovalRequirement,
}

impl Default for ToolAnnotations {
    fn default() -> Self {
        Self {
            execution: ToolExecution::Sequential,
            mutating: false,
            timeout: None,
            capabilities: vec![ToolCapability::ReadOnly],
            default_approval: ApprovalRequirement::Auto,
        }
    }
}

impl ToolAnnotations {
    /// Returns `true` if any capability implies mutation, or if the
    /// legacy `mutating` field is set to `true`.
    ///
    /// Checks both the capabilities-derived value AND the legacy field
    /// for backward compatibility (tests and examples may set only
    /// `mutating` without updating capabilities).
    #[must_use]
    pub fn is_mutating(&self) -> bool {
        self.mutating || self.capabilities.iter().any(|c| c.is_mutating())
    }

    /// Returns `true` if any capability implies network access.
    #[must_use]
    pub fn is_network(&self) -> bool {
        self.capabilities.iter().any(|c| c.is_network())
    }

    /// Returns `true` if any capability implies sandboxability.
    #[must_use]
    pub fn is_sandboxable(&self) -> bool {
        self.capabilities.iter().any(|c| c.is_sandboxable())
    }

    /// Create a new `ToolAnnotations` from capabilities, deriving `mutating`
    /// automatically.  This is the preferred constructor going forward.
    ///
    /// Reference: CodeWhale's `ToolSpec` trait which derives
    /// `is_read_only` / `is_sandboxable` / `supports_parallel` from capabilities.
    #[must_use]
    pub fn from_capabilities(
        execution: ToolExecution,
        capabilities: Vec<ToolCapability>,
        timeout: Option<std::time::Duration>,
        default_approval: ApprovalRequirement,
    ) -> Self {
        let mutating = capabilities.iter().any(|c| c.is_mutating());
        Self {
            execution,
            mutating,
            timeout,
            capabilities,
            default_approval,
        }
    }
}

/// Whether a [`Tool`] is safe to execute concurrently with other tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecution {
    /// Tool invocations must be serialized (no concurrent calls).
    Sequential,
    /// Tool invocations may run in parallel safely.
    ParallelSafe,
}

/// What kind of side effects a [`Tool`] has.
///
/// Reference: CodeWhale's `ToolCapability` enum with ReadOnly / WritesFiles /
/// ExecutesCode / Network / Sandboxable / RequiresApproval variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCapability {
    /// The tool only reads state and has no side effects.
    ReadOnly,
    /// The tool can write or modify files on disk.
    WritesFiles,
    /// The tool can execute arbitrary code or shell commands.
    ExecutesCode,
    /// The tool can make network requests.
    Network,
    /// The tool can be run in a sandbox for isolation.
    Sandboxable,
    /// The tool inherently requires user approval before execution.
    RequiresApproval,
}

impl ToolCapability {
    /// Returns `true` if the capability implies the tool mutates state.
    #[must_use]
    pub fn is_mutating(self) -> bool {
        matches!(
            self,
            ToolCapability::WritesFiles | ToolCapability::ExecutesCode
        )
    }

    /// Returns `true` if the capability implies the tool makes network requests.
    #[must_use]
    pub fn is_network(self) -> bool {
        matches!(self, ToolCapability::Network)
    }

    /// Returns `true` if the capability implies the tool can be sandboxed.
    #[must_use]
    pub fn is_sandboxable(self) -> bool {
        matches!(self, ToolCapability::Sandboxable | ToolCapability::ExecutesCode)
    }
}

/// Default approval level required before a [`Tool`] can execute.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRequirement {
    /// Automatically approved without user interaction.
    Auto,
    /// The user is prompted to approve or reject.
    Suggest,
    /// The user must explicitly approve before execution.
    Required,
}

/// Arguments passed to [`Tool::call`] for a single invocation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolInvocation {
    /// Unique identifier for this invocation (used to correlate results).
    pub call_id: String,
    /// The name of the tool being invoked.
    pub name: ToolName,
    /// JSON arguments matching the tool's `input_schema`.
    pub arguments: Value,
}

/// Errors that can occur during tool registration or execution.
#[derive(Debug, Error)]
pub enum ToolError {
    /// The requested tool name does not exist in the registry.
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    /// A tool with the same name already exists in the registry.
    #[error("duplicate tool: {0}")]
    DuplicateTool(String),
    /// The invocation arguments did not match the tool's schema.
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    /// The tool's execution failed with an internal error.
    #[error("tool execution failed: {0}")]
    Execution(String),
    /// The tool invocation exceeded its configured timeout.
    #[error("tool timed out")]
    Timeout,
    /// The tool invocation was cancelled before completion.
    #[error("tool was cancelled")]
    Cancelled,
}

/// A registry that manages a set of named [`Tool`]s.
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<ToolName, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Creates an empty [`ToolRegistry`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a [`Tool`] into the registry, wrapping it in `Arc`.
    ///
    /// Returns [`ToolError::DuplicateTool`] if a tool with the same name
    /// already exists.
    pub fn insert<T>(&mut self, tool: T) -> Result<(), ToolError>
    where
        T: Tool + 'static,
    {
        self.insert_arc(Arc::new(tool))
    }

    /// Inserts an already-`Arc`-wrapped [`Tool`] into the registry.
    ///
    /// Returns [`ToolError::DuplicateTool`] if a tool with the same name
    /// already exists.
    pub fn insert_arc(&mut self, tool: Arc<dyn Tool>) -> Result<(), ToolError> {
        let definition = tool.definition();
        if self.tools.contains_key(&definition.name) {
            return Err(ToolError::DuplicateTool(definition.name.to_string()));
        }
        self.tools.insert(definition.name, tool);
        Ok(())
    }

    /// Retrieves a [`Tool`] by name, if it exists.
    #[must_use]
    pub fn get(&self, name: &ToolName) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Returns the [`ToolDefinition`] for every registered tool.
    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

    /// Invokes a tool by its [`ToolInvocation`], respecting timeouts.
    ///
    /// Returns a [`ToolResult`] — tool errors and timeouts are converted
    /// into error results rather than panicking.
    pub async fn call(&self, invocation: ToolInvocation) -> ToolResult {
        let Some(tool) = self.get(&invocation.name) else {
            return ToolResult::error(
                invocation.call_id,
                invocation.name.to_string(),
                ToolError::UnknownTool(invocation.name.to_string()).to_string(),
            );
        };
        let definition = tool.definition();
        let call_id = invocation.call_id.clone();
        let name = invocation.name.to_string();
        let result = match definition.annotations.timeout {
            Some(timeout) => match time::timeout(timeout, tool.call(invocation)).await {
                Ok(result) => result,
                Err(_) => Err(ToolError::Timeout),
            },
            None => tool.call(invocation).await,
        };
        match result {
            Ok(execution) => ToolResult::ok(call_id, name, execution.output),
            Err(error) => ToolResult::error(call_id, name, error.to_string()),
        }
    }
}

/// The output produced by a single [`Tool`] invocation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolOutput {
    /// The JSON value returned by the tool.
    pub output: Value,
}

impl ToolOutput {
    /// Creates a new [`ToolOutput`] wrapping the given JSON value.
    #[must_use]
    pub fn new(output: Value) -> Self {
        Self { output }
    }
}

/// A [`Tool`] implementation backed by a closure.
#[derive(Clone)]
pub struct ToolFn<F> {
    definition: ToolDefinition,
    handler: F,
}

impl<F> ToolFn<F> {
    /// Creates a new [`ToolFn`] from a [`ToolDefinition`] and a handler closure.
    #[must_use]
    pub fn new(definition: ToolDefinition, handler: F) -> Self {
        Self {
            definition,
            handler,
        }
    }
}

impl<F> Tool for ToolFn<F>
where
    F: Fn(ToolInvocation) -> ToolFuture<'static> + Send + Sync,
{
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        (self.handler)(invocation)
    }
}
