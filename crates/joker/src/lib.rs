#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

mod agent;
mod context;
mod error;
mod event;
mod model;
mod policy;
mod protocol;
mod session;
mod tool;
mod web_search;

pub use agent::{
    Agent, AgentBuilder, AgentConfig, ExecutionMode, RunLimits, RunOutcome, RunRequest, ToolSet,
};
pub use context::{
    BuiltContext, ContextBuilder, ContextError, ContextFuture, ContextInput, ContextLimits,
    FixedWindowContextBuilder, PassthroughContextBuilder, SummaryContextBuilder,
};
pub use error::RunError;
pub use event::{Event, NoopObserver, Observer, ObserverFuture, RecordingObserver};
pub use model::{
    Model, ModelError, ModelFuture, ModelRequest, ModelResponseEvent, ModelStream, ScriptedModel,
    ScriptedStep,
};
pub use policy::{
    AllowAllPolicy, ApprovalRequest, ApprovalResponse, DenyAllMutatingPolicy, PermissionPolicy,
    PermissionRule, PolicyFuture, RulePattern, SharedApprovalChannel, ToolCategory, ToolDecision,
    ToolPolicy, ToolPolicyRequest,
};
pub use protocol::{
    Content, Conversation, Message, ReasoningContent, Role, StopReason, TextContent, ToolCall,
    ToolResult, Usage,
};
pub use tool::{
    Tool, ToolAnnotations, ToolDefinition, ToolError, ToolExecution, ToolFn, ToolFuture,
    ToolInvocation, ToolName, ToolOutput, ToolRegistry,
};
pub use web_search::{SearchFuture, SearchResult, WebSearch, WebSearchError};
pub use session::{
    JsonlSessionStore, SessionData, SessionError, SessionFuture, SessionInfo, SessionLoadFuture,
    SessionListFuture, SessionStore,
};
