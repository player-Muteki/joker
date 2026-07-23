use std::{collections::HashMap, sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::time;

use crate::{ToolResult, error::BoxFutureResult};

pub type ToolFuture<'a> = BoxFutureResult<'a, ToolOutput, ToolError>;

pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_>;
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolName(String);

impl ToolName {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: ToolName,
    pub description: String,
    pub input_schema: Value,
    pub annotations: ToolAnnotations,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolAnnotations {
    pub execution: ToolExecution,
    pub mutating: bool,
    pub timeout: Option<Duration>,
}

impl Default for ToolAnnotations {
    fn default() -> Self {
        Self {
            execution: ToolExecution::Sequential,
            mutating: false,
            timeout: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecution {
    Sequential,
    ParallelSafe,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub call_id: String,
    pub name: ToolName,
    pub arguments: Value,
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("duplicate tool: {0}")]
    DuplicateTool(String),
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("tool execution failed: {0}")]
    Execution(String),
    #[error("tool timed out")]
    Timeout,
    #[error("tool was cancelled")]
    Cancelled,
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<ToolName, Arc<dyn Tool>>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert<T>(&mut self, tool: T) -> Result<(), ToolError>
    where
        T: Tool + 'static,
    {
        self.insert_arc(Arc::new(tool))
    }

    pub fn insert_arc(&mut self, tool: Arc<dyn Tool>) -> Result<(), ToolError> {
        let definition = tool.definition();
        if self.tools.contains_key(&definition.name) {
            return Err(ToolError::DuplicateTool(definition.name.to_string()));
        }
        self.tools.insert(definition.name, tool);
        Ok(())
    }

    #[must_use]
    pub fn get(&self, name: &ToolName) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolOutput {
    pub output: Value,
}

impl ToolOutput {
    #[must_use]
    pub fn new(output: Value) -> Self {
        Self { output }
    }
}

#[derive(Clone)]
pub struct ToolFn<F> {
    definition: ToolDefinition,
    handler: F,
}

impl<F> ToolFn<F> {
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
