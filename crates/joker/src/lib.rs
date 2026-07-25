#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

mod agent;
mod agent_profiles;
mod context;
mod error;
mod event;
mod model;
mod permission_engine;
mod policy;
mod protocol;
mod credential;
mod session;
mod tool;
mod web_search;

pub use agent::{
    Agent, AgentBuilder, AgentConfig, AgentRuntime, ExecutionMode, Op, RetryConfig, RunLimits,
    RunOutcome, RunRequest, ToolSet, TurnOutcome,
};
pub use agent_profiles::{builtin_agent_profiles, builtin_constraint_file_content};
pub use context::{
    BuiltContext, CompactingContextBuilder, CompactionLevel, ContextBuilder, ContextError,
    ContextFuture, ContextInput, ContextLimits, ContextThresholds, FixedWindowContextBuilder,
    PassthroughContextBuilder, PrefixedContextBuilder, SummaryContextBuilder, assemble_system_prompt,
    estimate_tokens, micro_dedup_messages,
};
pub use error::RunError;
pub use event::{Event, NoopObserver, Observer, ObserverFuture, RecordingObserver};
pub use model::{
    Model, ModelError, ModelFuture, ModelRequest, ModelResponseEvent, ModelStream, ScriptedModel,
    ScriptedStep,
};
pub use permission_engine::{
    AgentPermission, PermissionDecision, PermissionEngine, PermissionSetting,
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
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError, ToolExecution, ToolFn, ToolFuture,
    ToolInvocation, ToolName, ToolOutput, ToolRegistry,
};
pub use web_search::{SearchFuture, SearchResult, WebSearch, WebSearchError};
pub use credential::{CredentialError, CredentialStore};
pub use session::{
    JsonlSessionStore, SessionData, SessionError, SessionFuture, SessionInfo, SessionLoadFuture,
    SessionListFuture, SessionStore,
};
