use std::sync::Arc;

use crate::{ToolDefinition, ToolInvocation, error::BoxFutureResult};

pub type PolicyFuture<'a> = BoxFutureResult<'a, ToolDecision, std::convert::Infallible>;

pub trait ToolPolicy: Send + Sync {
    fn evaluate<'a>(&'a self, request: ToolPolicyRequest<'a>) -> PolicyFuture<'a>;
}

#[derive(Clone, Debug)]
pub struct ToolPolicyRequest<'a> {
    pub invocation: &'a ToolInvocation,
    pub definition: Option<&'a ToolDefinition>,
}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolDecision {
    Allow,
    Deny { reason: String },
    Ask { request_id: String, reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub tool_name: String,
    pub subject: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalResponse {
    Approved { remember_for_session: bool },
    Denied { reason: String },
}

// ── Simple in-memory approval channel ───────────────────────────────────

use std::sync::Mutex;

/// A shared approval channel that supports one outstanding request at a time.
/// Both the agent and the UI share a reference to the same `Arc<SharedApprovalChannel>`.
#[derive(Clone, Default, Debug)]
pub struct SharedApprovalChannel {
    inner: Arc<Mutex<SharedApprovalState>>,
}

#[derive(Debug, Default)]
struct SharedApprovalState {
    pending: Option<ApprovalRequest>,
    response: Option<ApprovalResponse>,
}

impl SharedApprovalChannel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn submit(&self, request: ApprovalRequest) {
        let mut state = self.inner.lock().expect("approval channel lock");
        state.pending = Some(request);
        state.response = None;
    }

    pub fn respond(&self, response: ApprovalResponse) {
        let mut state = self.inner.lock().expect("approval channel lock");
        state.response = Some(response);
    }

    pub fn take_response(&self) -> Option<ApprovalResponse> {
        let mut state = self.inner.lock().expect("approval channel lock");
        let resp = state.response.take();
        if resp.is_some() {
            state.pending = None;
        }
        resp
    }

    pub fn pending_request(&self) -> Option<ApprovalRequest> {
        self.inner
            .lock()
            .expect("approval channel lock")
            .pending
            .clone()
    }
}

// ── Rule-based PermissionPolicy ─────────────────────────────────────────

/// A layered permission policy that evaluates tool calls against a set of rules.
///
/// Evaluation priority (highest wins):
/// 1. Hard deny rules (explicit `RuleDecision::Deny`)
/// 2. Persisted allow/deny rules
/// 3. Session allow/deny rules
/// 4. Agent profile rules
/// 5. Tool annotation default (mutating → Ask, read-only → Allow)
/// 6. Global default (Allow)
#[derive(Clone)]
pub struct PermissionPolicy {
    rules: Vec<PermissionRule>,
    session_allows: Vec<String>,
    persisted_allows: Vec<String>,
    default_for_mutating: ToolDecision,
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            session_allows: Vec::new(),
            persisted_allows: Vec::new(),
            default_for_mutating: ToolDecision::Ask {
                request_id: String::new(),
                reason: "mutating tool requires approval".into(),
            },
        }
    }
}

impl PermissionPolicy {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_rules(mut self, rules: Vec<PermissionRule>) -> Self {
        self.rules = rules;
        self
    }

    #[must_use]
    pub fn with_default_for_mutating(mut self, decision: ToolDecision) -> Self {
        self.default_for_mutating = decision;
        self
    }

    pub fn add_session_allow(&mut self, tool_name: impl Into<String>) {
        self.session_allows.push(tool_name.into());
    }

    pub fn add_persisted_allow(&mut self, tool_name: impl Into<String>) {
        self.persisted_allows.push(tool_name.into());
    }

    fn evaluate_rules(&self, request: &ToolPolicyRequest) -> Option<ToolDecision> {
        for rule in &self.rules {
            if rule.matches(request) {
                let decision = rule.decision.clone();
                match &decision {
                    // Hard deny / allow are terminal at this priority level
                    ToolDecision::Deny { .. } => return Some(decision),
                    ToolDecision::Allow => return Some(decision),
                    // Ask rules propagate up to be overridden by higher-priority rules
                    ToolDecision::Ask { .. } => {}
                }
            }
        }
        None
    }
}

impl ToolPolicy for PermissionPolicy {
    fn evaluate<'a>(&'a self, request: ToolPolicyRequest<'a>) -> PolicyFuture<'a> {
        Box::pin(async move {
            // Level 1: Hard deny / allow from rules (highest priority)
            if let Some(decision) = self.evaluate_rules(&request) {
                return Ok(decision);
            }

            // Level 2: Session allows
            let tool_name = request.invocation.name.as_str();
            if self.session_allows.iter().any(|a| a == tool_name) {
                return Ok(ToolDecision::Allow);
            }

            // Level 3: Persisted allows
            if self.persisted_allows.iter().any(|a| a == tool_name) {
                return Ok(ToolDecision::Allow);
            }

            // Level 4: Tool annotation default
            if let Some(definition) = request.definition {
                if definition.annotations.mutating {
                    let mut decision = self.default_for_mutating.clone();
                    if let ToolDecision::Ask { request_id, .. } = &mut decision {
                        *request_id = format!("ask-{}", tool_name);
                    }
                    return Ok(decision);
                }
            }

            // Level 5: Global default — Allow
            Ok(ToolDecision::Allow)
        })
    }
}

/// A single permission rule that matches against a tool invocation.
#[derive(Clone, Debug)]
pub struct PermissionRule {
    pub pattern: RulePattern,
    pub decision: ToolDecision,
}

impl PermissionRule {
    #[must_use]
    pub fn new(pattern: RulePattern, decision: ToolDecision) -> Self {
        Self { pattern, decision }
    }

    fn matches(&self, request: &ToolPolicyRequest) -> bool {
        self.pattern.matches(request)
    }
}

/// Patterns for matching tool invocations in permission rules.
#[derive(Clone, Debug)]
pub enum RulePattern {
    /// Exact tool name match (e.g. "write_file")
    ToolName(String),
    /// Tool category match (e.g. Read, Write, Shell, Network)
    ToolCategory(ToolCategory),
    /// Path prefix match for file tools (e.g. "docs/")
    PathPrefix(String),
    /// Command prefix match for shell tools (e.g. "git status")
    CommandPrefix(String),
}

impl RulePattern {
    fn matches(&self, request: &ToolPolicyRequest) -> bool {
        let tool_name = request.invocation.name.as_str();
        match self {
            RulePattern::ToolName(name) => tool_name == name,
            RulePattern::ToolCategory(category) => category.matches(tool_name),
            RulePattern::PathPrefix(prefix) => {
                request
                    .invocation
                    .arguments
                    .get("path")
                    .and_then(|v| v.as_str())
                    .map(|p| p.starts_with(prefix))
                    .unwrap_or(false)
            }
            RulePattern::CommandPrefix(prefix) => {
                request
                    .invocation
                    .arguments
                    .get("command")
                    .and_then(|v| v.as_str())
                    .map(|c| c.trim_start().starts_with(prefix))
                    .unwrap_or(false)
            }
        }
    }
}

/// Categories for classifying tools in permission rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolCategory {
    Read,
    Write,
    Shell,
    Network,
}

impl ToolCategory {
    fn matches(&self, tool_name: &str) -> bool {
        match self {
            ToolCategory::Read => matches!(
                tool_name,
                "read_file" | "list_files" | "grep" | "read"
            ),
            ToolCategory::Write => matches!(
                tool_name,
                "write_file" | "edit_file" | "apply_patch" | "create_file"
            ),
            ToolCategory::Shell => {
                tool_name == "shell" || tool_name == "bash" || tool_name == "exec_shell"
            }
            ToolCategory::Network => {
                tool_name == "web_search"
                    || tool_name == "fetch_url"
                    || tool_name == "http_get"
                    || tool_name.starts_with("mcp_")
            }
        }
    }
}

// ── Simple policy implementations ───────────────────────────────────────

#[derive(Default)]
pub struct AllowAllPolicy;

impl ToolPolicy for AllowAllPolicy {
    fn evaluate<'a>(&'a self, _request: ToolPolicyRequest<'a>) -> PolicyFuture<'a> {
        Box::pin(async { Ok(ToolDecision::Allow) })
    }
}

#[derive(Default)]
pub struct DenyAllMutatingPolicy;

impl ToolPolicy for DenyAllMutatingPolicy {
    fn evaluate<'a>(&'a self, request: ToolPolicyRequest<'a>) -> PolicyFuture<'a> {
        Box::pin(async move {
            match request.definition {
                Some(definition) if definition.annotations.mutating => Ok(ToolDecision::Deny {
                    reason: "mutating tools are denied".into(),
                }),
                _ => Ok(ToolDecision::Allow),
            }
        })
    }
}
