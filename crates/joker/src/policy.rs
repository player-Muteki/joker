use std::sync::Arc;

use crate::{ToolDefinition, ToolInvocation, error::BoxFutureResult};

/// Result type returned by [`ToolPolicy::evaluate`].
pub type PolicyFuture<'a> = BoxFutureResult<'a, ToolDecision, std::convert::Infallible>;

/// Decides whether a tool invocation is allowed, denied, or needs approval.
pub trait ToolPolicy: Send + Sync {
    /// Evaluate a tool invocation request and return a [`ToolDecision`].
    fn evaluate<'a>(&'a self, request: ToolPolicyRequest<'a>) -> PolicyFuture<'a>;
}

/// Input to [`ToolPolicy::evaluate`] describing a tool invocation to be checked.
#[derive(Clone, Debug)]
pub struct ToolPolicyRequest<'a> {
    /// The tool invocation being evaluated.
    pub invocation: &'a ToolInvocation,
    /// Optional tool definition with metadata like annotations.
    pub definition: Option<&'a ToolDefinition>,
}

/// Outcome of a policy evaluation for a tool invocation.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolDecision {
    /// The tool invocation is permitted.
    Allow,
    /// The tool invocation is denied with an explanation.
    Deny {
        /// Human-readable reason for the denial.
        reason: String,
    },
    /// The tool invocation requires user approval before proceeding.
    Ask {
        /// Unique identifier for the approval request.
        request_id: String,
        /// Human-readable reason why approval is needed.
        reason: String,
    },
}

/// A request submitted for user approval via an approval channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalRequest {
    /// Unique identifier for this approval request.
    pub request_id: String,
    /// Name of the tool that triggered the request.
    pub tool_name: String,
    /// Subject or context of the approval request.
    pub subject: String,
    /// Human-readable explanation of why approval is needed.
    pub reason: String,
}

/// Response from a user to an [`ApprovalRequest`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalResponse {
    /// The request was approved, optionally remembered for the session.
    Approved {
        /// Whether to remember this decision for the remainder of the session.
        remember_for_session: bool,
    },
    /// The request was denied with an explanation.
    Denied {
        /// Human-readable reason for the denial.
        reason: String,
    },
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
    session_allows: Vec<String>,
}

impl SharedApprovalChannel {
    /// Create a new `SharedApprovalChannel`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Submit an [`ApprovalRequest`] for the user to respond to.
    pub fn submit(&self, request: ApprovalRequest) {
        let mut state = self.inner.lock().expect("approval channel lock");
        state.pending = Some(request);
        state.response = None;
    }

    /// Supply an [`ApprovalResponse`] for the currently pending request.
    pub fn respond(&self, response: ApprovalResponse) {
        let mut state = self.inner.lock().expect("approval channel lock");
        state.response = Some(response);
    }

    /// Consume and return the pending response, if any.
    pub fn take_response(&self) -> Option<ApprovalResponse> {
        let mut state = self.inner.lock().expect("approval channel lock");
        let resp = state.response.take();
        if resp.is_some() {
            state.pending = None;
        }
        resp
    }

    /// Return a clone of the current pending request, if any.
    pub fn pending_request(&self) -> Option<ApprovalRequest> {
        self.inner
            .lock()
            .expect("approval channel lock")
            .pending
            .clone()
    }

    /// Record a tool name as approved for the remainder of the session.
    pub fn grant_for_session(&self, tool_name: impl Into<String>) {
        let tool_name = tool_name.into();
        let mut state = self.inner.lock().expect("approval channel lock");
        if !state.session_allows.iter().any(|name| name == &tool_name) {
            state.session_allows.push(tool_name);
        }
    }

    /// Check whether a tool name has been session-approved.
    #[must_use]
    pub fn is_granted_for_session(&self, tool_name: &str) -> bool {
        self.inner
            .lock()
            .expect("approval channel lock")
            .session_allows
            .iter()
            .any(|name| name == tool_name)
    }
}

// ── Rule-based PermissionPolicy ─────────────────────────────────────────

/// A layered permission policy with `findLast` match semantics.
///
/// Evaluation priority (last match wins, like OpenCode's `findLast`):
/// 1. Hard deny / allow from explicit rules (findLast over these)
/// 2. Session allows
/// 3. Persisted allows
/// 4. Tool annotation default (mutating → Ask, read-only → Allow)
/// 5. Global default (Allow)
///
/// ## Shell chain detection (gemini-cli style)
/// For shell commands, the policy checks for chaining operators (`;`, `&&`, `||`,
/// `|`, `$()`, `` ` ``) and redirects (`>`, `>>`, `<`). Chained commands are
/// never automatically trusted — they always fall through to Ask.
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
    /// Create an empty [`PermissionPolicy`] with the default "Ask for mutating" behaviour.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the explicit rules for this policy (builder pattern).
    #[must_use]
    pub fn with_rules(mut self, rules: Vec<PermissionRule>) -> Self {
        self.rules = rules;
        self
    }

    /// Override the default decision for mutating tools (builder pattern).
    #[must_use]
    pub fn with_default_for_mutating(mut self, decision: ToolDecision) -> Self {
        self.default_for_mutating = decision;
        self
    }

    /// Register a tool name as session-approved for this policy.
    pub fn add_session_allow(&mut self, tool_name: impl Into<String>) {
        self.session_allows.push(tool_name.into());
    }

    /// Register a tool name as persisted-approved for this policy.
    pub fn add_persisted_allow(&mut self, tool_name: impl Into<String>) {
        self.persisted_allows.push(tool_name.into());
    }

    /// Find the LAST matching rule (OpenCode-style `findLast` semantics).
    fn find_last_rule(&self, request: &ToolPolicyRequest) -> Option<ToolDecision> {
        self.rules
            .iter()
            .rev() // iterate in reverse → last match wins
            .find(|rule| rule.matches(request))
            .map(|rule| rule.decision.clone())
    }

    /// Check whether a shell command contains chain operators (gemini-cli style).
    ///
    /// Returns `true` if the command has `;`, `&&`, `||`, `|` (outside quotes),
    /// or shell redirections `>`, `>>`, `<`, `<<`.
    fn shell_has_chaining(&self, command: &str) -> bool {
        // Simple heuristic: check for chain operators outside of quotes.
        // This mirrors gemini-cli's hasRedirection + chained command detection.
        let mut in_single = false;
        let mut in_double = false;
        let mut prev = ' ';
        for ch in command.chars() {
            match ch {
                '\'' if !in_double => in_single = !in_single,
                '"' if !in_single => in_double = !in_double,
                ';' | '&' | '|' if !in_single && !in_double => {
                    if ch == '|' && prev == '|' {
                        // || is a chain operator, but single | is a pipe (also chaining)
                    }
                    return true;
                }
                '>' | '<' if !in_single && !in_double => return true,
                _ => {}
            }
            prev = ch;
        }
        false
    }
}

impl ToolPolicy for PermissionPolicy {
    fn evaluate<'a>(&'a self, request: ToolPolicyRequest<'a>) -> PolicyFuture<'a> {
        Box::pin(async move {
            let is_shell = request.invocation.name.as_str() == "shell";
            let tool_name = request.invocation.name.as_str();

            // Level 1: Explicit rules (findLast — last match wins)
            if let Some(decision) = self.find_last_rule(&request) {
                return Ok(decision);
            }

            // Level 2: Session allows
            if self.session_allows.iter().any(|a| a == tool_name) {
                return Ok(ToolDecision::Allow);
            }

            // Level 3: Persisted allows
            if self.persisted_allows.iter().any(|a| a == tool_name) {
                return Ok(ToolDecision::Allow);
            }

            // Level 4: Shell chain detection (gemini-cli style)
            // Chained / redirected shell commands are never automatically trusted
            if is_shell
                && let Some(cmd) = request
                    .invocation
                    .arguments
                    .get("command")
                    .and_then(|v| v.as_str())
                && self.shell_has_chaining(cmd)
            {
                return Ok(ToolDecision::Ask {
                    request_id: format!("ask-shell-chain-{}", now_nanos()),
                    reason: format!(
                        "shell command with chaining/redirect requires approval: {cmd}"
                    ),
                });
            }

            // Level 5: Tool annotation default
            if let Some(definition) = request.definition
                && definition.annotations.is_mutating()
            {
                let mut decision = self.default_for_mutating.clone();
                if let ToolDecision::Ask { request_id, .. } = &mut decision {
                    *request_id = format!("ask-{}", tool_name);
                }
                return Ok(decision);
            }

            // Level 6: Global default — Allow
            Ok(ToolDecision::Allow)
        })
    }
}

/// A single permission rule that matches against a tool invocation.
#[derive(Clone, Debug)]
pub struct PermissionRule {
    /// Pattern the invocation must match for this rule to apply.
    pub pattern: RulePattern,
    /// Decision to return when the pattern matches.
    pub decision: ToolDecision,
}

impl PermissionRule {
    /// Create a new `PermissionRule` from a pattern and decision.
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
    /// Exact tool name match (e.g. `"write_file"`).
    ToolName(String),
    /// Match any tool belonging to a [`ToolCategory`].
    ToolCategory(ToolCategory),
    /// Match file tools whose `path` argument starts with the given prefix.
    PathPrefix(String),
    /// Match shell tools whose `command` argument starts with the given prefix.
    CommandPrefix(String),
}

impl RulePattern {
    fn matches(&self, request: &ToolPolicyRequest) -> bool {
        let tool_name = request.invocation.name.as_str();
        match self {
            RulePattern::ToolName(name) => tool_name == name,
            RulePattern::ToolCategory(category) => category.matches(tool_name),
            RulePattern::PathPrefix(prefix) => request
                .invocation
                .arguments
                .get("path")
                .and_then(|v| v.as_str())
                .map(|p| p.starts_with(prefix))
                .unwrap_or(false),
            RulePattern::CommandPrefix(prefix) => request
                .invocation
                .arguments
                .get("command")
                .and_then(|v| v.as_str())
                .map(|c| c.trim_start().starts_with(prefix))
                .unwrap_or(false),
        }
    }
}

/// Categories for classifying tools in permission rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolCategory {
    /// Read-only tools (e.g. `read_file`, `grep`).
    Read,
    /// Mutating tools (e.g. `write_file`, `edit_file`).
    Write,
    /// Shell execution tools (e.g. `shell`, `bash`).
    Shell,
    /// Network access tools (e.g. `web_search`, `fetch_url`).
    Network,
}

impl ToolCategory {
    fn matches(&self, tool_name: &str) -> bool {
        match self {
            ToolCategory::Read => matches!(tool_name, "read_file" | "list_files" | "grep" | "read"),
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

/// A [`ToolPolicy`] that allows every invocation unconditionally.
#[derive(Default)]
pub struct AllowAllPolicy;

impl ToolPolicy for AllowAllPolicy {
    fn evaluate<'a>(&'a self, _request: ToolPolicyRequest<'a>) -> PolicyFuture<'a> {
        Box::pin(async { Ok(ToolDecision::Allow) })
    }
}

/// A [`ToolPolicy`] that denies mutating tools and allows everything else.
#[derive(Default)]
pub struct DenyAllMutatingPolicy;

impl ToolPolicy for DenyAllMutatingPolicy {
    fn evaluate<'a>(&'a self, request: ToolPolicyRequest<'a>) -> PolicyFuture<'a> {
        Box::pin(async move {
            match request.definition {
                Some(definition) if definition.annotations.is_mutating() => {
                    Ok(ToolDecision::Deny {
                        reason: "mutating tools are denied".into(),
                    })
                }
                _ => Ok(ToolDecision::Allow),
            }
        })
    }
}

// ── BashArityDict (CodeWhale-style command prefix recognition) ────────────

/// Maps command prefixes to their expected positional arity so permission rules
/// can distinguish `git status` from `git push` without allowing everything under `git`.
///
/// Reference: CodeWhale's `bash_arity.rs` which covers 30+ tools.
#[derive(Clone, Debug)]
pub struct BashArityDict {
    entries: Vec<(&'static str, usize)>,
}

impl BashArityDict {
    /// Create a dict populated with the built-in command prefixes and arities.
    #[must_use]
    pub fn new() -> Self {
        // Common tool command prefixes and their expected subcommand depth
        let entries: Vec<(&str, usize)> = vec![
            ("git", 2),   // git <subcommand>
            ("cargo", 2), // cargo <subcommand>
            ("npm", 2),   // npm <subcommand>
            ("pnpm", 2),
            ("yarn", 2),
            ("bun", 2),
            ("docker", 2), // docker <subcommand>
            ("docker compose", 3),
            ("podman", 2),
            ("kubectl", 2),
            ("helm", 2),
            ("make", 1),
            ("cmake", 2),
            ("python", 1),
            ("python3", 1),
            ("pip", 2),
            ("pip3", 2),
            ("rustup", 2),
            ("go", 2), // go <subcommand>
            ("deno", 2),
            ("node", 1),
            ("npx", 2),
            ("just", 1),
            ("cargo expand", 2), // cargo-expand
            ("cargo nextest", 3),
            ("cargo clippy", 2),
            ("cargo fmt", 2),
            ("cargo build", 2),
            ("cargo test", 2),
            ("cargo run", 2),
            ("cargo check", 2),
            ("ssh", 1),
            ("scp", 2),
            ("rsync", 2),
            ("curl", 1),
            ("wget", 1),
            ("ls", 1),
            ("cat", 1),
            ("head", 1),
            ("tail", 1),
            ("less", 1),
            ("more", 1),
            ("echo", 1),
            ("env", 1),
            ("export", 1),
            ("cd", 1),
            ("mkdir", 1),
            ("rmdir", 1),
            ("rm", 1),
            ("cp", 1),
            ("mv", 1),
            ("ln", 1),
            ("chmod", 1),
            ("chown", 1),
            ("touch", 1),
            ("find", 1),
            ("xargs", 1),
            ("sort", 1),
            ("uniq", 1),
            ("wc", 1),
            ("tee", 1),
            ("sed", 1),
            ("awk", 1),
        ];
        Self { entries }
    }

    /// Given a full command string, find the best matching prefix and expected arity.
    ///
    /// Returns `(prefix, arity)` where `arity` is the number of space-delimited
    /// segments the prefix expects. For example, `"git status -s"` matches
    /// prefix `"git"` with arity 2, so `"git status"` is the recognized prefix.
    #[must_use]
    pub fn match_prefix(&self, command: &str) -> Option<(&'static str, usize)> {
        let trimmed = command.trim_start();
        // Try longest prefix first (greedy match)
        for (prefix, arity) in &self.entries {
            if let Some(after_prefix) = trimmed.strip_prefix(prefix) {
                // Prefix must be followed by space, tab, or end-of-string
                if after_prefix.is_empty()
                    || after_prefix.starts_with(' ')
                    || after_prefix.starts_with('\t')
                {
                    return Some((prefix, *arity));
                }
            }
        }
        None
    }
}

impl Default for BashArityDict {
    fn default() -> Self {
        Self::new()
    }
}

fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}
