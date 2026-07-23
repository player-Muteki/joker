#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

mod agent;
mod context;
mod error;
mod event;
mod model;
mod policy;
mod protocol;
mod tool;

pub use agent::{Agent, AgentConfig, ExecutionMode, RunLimits, RunOutcome, RunRequest};
pub use context::{
    BuiltContext, ContextBuilder, ContextError, ContextFuture, ContextInput, ContextLimits,
    FixedWindowContextBuilder, PassthroughContextBuilder,
};
pub use error::RunError;
pub use event::{Event, NoopObserver, Observer, ObserverFuture, RecordingObserver};
pub use model::{
    Model, ModelError, ModelFuture, ModelRequest, ModelResponseEvent, ModelStream, ScriptedModel,
    ScriptedStep,
};
pub use policy::{
    AllowAllPolicy, DenyAllMutatingPolicy, PolicyFuture, ToolDecision, ToolPolicy,
    ToolPolicyRequest,
};
pub use protocol::{
    Content, Conversation, Message, ReasoningContent, Role, StopReason, TextContent, ToolCall,
    ToolResult, Usage,
};
pub use tool::{
    Tool, ToolAnnotations, ToolDefinition, ToolError, ToolExecution, ToolFn, ToolFuture,
    ToolInvocation, ToolName, ToolOutput, ToolRegistry,
};
