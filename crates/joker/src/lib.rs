//! Joker — API-first Rust coding agent.
//!
//! Joker is a TUI-interactive coding agent with a library-first architecture.
//! The core library provides:
//! - **Agent loop**: [`Agent`], [`AgentRuntime`], [`Op`]-driven event loop
//! - **Tool system**: [`Tool`] trait, [`ToolRegistry`], permission gating
//! - **Provider abstraction**: [`Model`] trait with OpenAI/Anthropic/Gemini backends
//! - **Session management**: [`SessionStore`] for persistence
//! - **Context building**: multi-strategy context compression and assembly
//! - **Event system**: [`Event`], [`Observer`] for streaming turn events
//! - **Policy engine**: [`PermissionEngine`], [`ToolPolicy`], [`AllowAllPolicy`]

#![forbid(unsafe_code)]
#![deny(unreachable_pub)]
#![warn(missing_docs)]

mod agent;
mod agent_config;
mod agent_profiles;
mod agent_runtime;
mod agent_types;
mod context;
mod credential;
mod error;
/// Turn-level events emitted during agent execution.
pub mod event;
/// Lifecycle hooks for session/turn/tool events.
pub mod hook;
mod message_queue;
mod model;
mod permission_engine;
mod policy;
mod protocol;
mod session;
/// Skills inject prompt fragments based on file-path patterns.
pub mod skill;
mod tool;
mod tool_set;
mod web_search;

pub use agent::{Agent, AgentBuilder};
pub use agent_config::{AgentConfig, ExecutionMode, RetryConfig, RunLimits};
pub use agent_profiles::{builtin_agent_profiles, builtin_constraint_file_content};
pub use agent_runtime::{AgentRuntime, Op};
pub use agent_types::{RunOutcome, RunRequest, TurnOutcome};
pub use context::{
    BuiltContext, CompactingContextBuilder, CompactionLevel, ContextBuilder, ContextError,
    ContextFuture, ContextInput, ContextLimits, ContextThresholds, FixedWindowContextBuilder,
    PassthroughContextBuilder, PrefixedContextBuilder, SummaryContextBuilder, assemble_system_prompt,
    estimate_tokens, micro_dedup_messages,
};
pub use credential::{CredentialError, CredentialStore};
pub use error::RunError;
pub use event::{Event, NoopObserver, Observer, ObserverFuture, RecordingObserver};
pub use hook::{Hook, HookRegistry};
pub use message_queue::{DrainMode, PendingMessageQueue};
pub use model::{
    Model, ModelError, ModelFuture, ModelRequest, ModelResponseEvent, ModelStream, ScriptedModel,
    ScriptedStep,
};
pub use permission_engine::{
    AgentPermission, PermissionDecision, PermissionEngine, PermissionSetting,
};
pub use policy::{
    AllowAllPolicy, ApprovalRequest, ApprovalResponse, BashArityDict, DenyAllMutatingPolicy,
    PermissionPolicy, PermissionRule, PolicyFuture, RulePattern, SharedApprovalChannel,
    ToolCategory, ToolDecision, ToolPolicy, ToolPolicyRequest,
};
pub use protocol::{
    Content, Conversation, Message, ReasoningContent, Role, StopReason, TextContent, ToolCall,
    ToolResult, Usage,
};
pub use session::{
    JsonlSessionStore, SessionData, SessionError, SessionFuture, SessionInfo, SessionLoadFuture,
    SessionListFuture, SessionStore,
};
pub use skill::{Skill, SkillRegistry};
pub use tool::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFn, ToolFuture, ToolInvocation, ToolName, ToolOutput, ToolRegistry,
};
pub use tool_set::ToolSet;
pub use web_search::{SearchFuture, SearchResult, WebSearch, WebSearchError};
