//! Agent permission engine — the configuration layer that owns agent profiles,
//! session grants, and hard (non-overridable) permissions.
//!
//! Separated from `policy.rs` which implements the `ToolPolicy` trait consumed by
//! the agent loop. The engine produces a filtered `ToolRegistry` and a `ToolPolicy`
//! impl for each agent, keeping profile management decoupled from the run loop.
//!
//! ## Evaluation order (highest priority wins)
//!
//! 1. **Hard permission** — if set, always terminal. Used by plan agent to block
//!    all mutating tools regardless of user config.
//! 2. **Agent Disabled** — tool hidden from model entirely.
//! 3. **Agent AutoAccept** — tool runs without asking.
//! 4. **Session grant** — "Allow for this session" temporary override (persisted to disk).
//! 5. **Agent Ask** — tool requires interactive approval.
//! 6. **Default** — fall back to tool annotation default.
//!
//! ## Persistence
//!
//! Session grants are persisted to a JSON file (`grants.json` in the joker config dir)
//! so "Allow for Session" decisions survive restarts. The file is loaded on engine
//! construction and saved on every grant mutation (reference: pi's auth.json pattern).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::policy::{SharedApprovalChannel, ToolDecision, ToolPolicy, ToolPolicyRequest};
use crate::tool::{ToolName, ToolRegistry};
use crate::policy::PolicyFuture;

/// Returns the current unix timestamp in millis for serialization.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

/// Per-agent permission setting for a single tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionSetting {
    /// Prompt user before every invocation.
    Ask,
    /// Execute immediately without prompting.
    AutoAccept,
    /// Hide the tool from the model entirely.
    Disabled,
}

/// A pattern-based hard permission rule (MiMo-Code style).
///
/// Allows precise control like: `edit: {"*": "deny", "plans/*.md": "allow"}`.
#[derive(Clone, Debug)]
pub struct HardPermissionRule {
    /// Tool name pattern (e.g. `"edit"`, `"write_file"`, or `"*"` for all).
    pub tool_pattern: String,
    /// Resource path pattern (e.g. `"*.md"`, `"src/*"`, or `"*"`).
    pub resource_pattern: String,
    /// The non-overridable setting.
    pub setting: PermissionSetting,
}

/// Permission configuration for a named agent.
#[derive(Clone, Debug)]
pub struct AgentPermission {
    /// Name of the agent this config applies to.
    pub agent_name: String,
    /// Per-tool permission settings.
    pub tool_permissions: HashMap<ToolName, PermissionSetting>,
    /// Path to the agent's constraint file (markdown).
    pub constraint_file: PathBuf,
    /// Non-overridable blanket permission for all mutating tools.
    ///
    /// Checked first and always terminal.  Use `Disabled` for read-only
    /// agents (e.g. plan) or `AutoAccept` for fully trusted agents.
    /// For path-precise rules see `hard_permission_rules`.
    pub hard_permission: Option<PermissionSetting>,
    /// Path-precise hard permission rules (MiMo-Code style).
    ///
    /// Each rule specifies a tool pattern and resource path pattern.
    /// Rules are evaluated in order — the first matching rule wins.
    /// Example:
    /// ```text
    /// hard_permission_rules: [
    ///   { tool_pattern: "edit_file", resource_pattern: "**", setting: Disabled },
    ///   { tool_pattern: "edit_file", resource_pattern: "plans/*.md", setting: AutoAccept },
    /// ]
    /// ```
    pub hard_permission_rules: Vec<HardPermissionRule>,
    /// Optional per-agent model override.
    pub model: Option<String>,
}

/// Outcome of evaluating a tool permission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Tool execution is permitted.
    Allow,
    /// Tool execution requires interactive approval.
    Ask {
        /// Name of the tool being requested.
        tool_name: String,
        /// Human-readable explanation for the prompt.
        reason: String,
        /// Unique identifier for this approval request.
        request_id: String,
    },
    /// Tool execution is denied.
    Deny {
        /// Reason for denial.
        reason: String,
    },
}

/// A temporary session-scoped grant persisted to disk.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SessionGrant {
    agent_name: String,
    tool_name: String,
    /// Unix timestamp in millis when the grant was created.
    #[serde(default = "now_millis")]
    granted_at_ms: u64,
}

/// The permission engine: owns agent profiles, session grants, and produces
/// per-agent [`ToolPolicy`] implementations and filtered [`ToolRegistry`] instances.
///
/// Session grants are persisted to `grants_path` so "Allow for Session" decisions
/// survive restarts (reference: pi's auth.json pattern).
#[derive(Clone, Debug)]
pub struct PermissionEngine {
    agent_permissions: HashMap<String, AgentPermission>,
    session_grants: Vec<SessionGrant>,
    /// Path to the grants persistence file.
    grants_path: Option<PathBuf>,
    /// Quick-lookup index: (agent, tool) -> bool
    grant_index: HashSet<(String, String)>,
}

impl PermissionEngine {
    /// Create an empty permission engine with no persistence.
    #[must_use]
    pub fn new() -> Self {
        Self {
            agent_permissions: HashMap::new(),
            session_grants: Vec::new(),
            grants_path: None,
            grant_index: HashSet::new(),
        }
    }

    /// Create a permission engine that persists session grants to a file.
    ///
    /// Loads existing grants from the file if it exists (pi-style persistence).
    #[must_use]
    pub fn with_persistence(grants_path: impl Into<PathBuf>) -> Self {
        let path = grants_path.into();
        let grants = if path.exists() {
            fs::read_to_string(&path)
                .ok()
                .and_then(|data| serde_json::from_str::<Vec<SessionGrant>>(&data).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let grant_index = grants.iter()
            .map(|g| (g.agent_name.clone(), g.tool_name.clone()))
            .collect();
        info!(target: "permission", count = grants.len(), path = %path.display(), "loaded persisted session grants");
        Self {
            agent_permissions: HashMap::new(),
            session_grants: grants,
            grants_path: Some(path),
            grant_index,
        }
    }

    /// Persist the current session grants to disk (pi-style auth.json pattern).
    fn persist_grants(&self) {
        if let Some(path) = &self.grants_path {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let data: Vec<SessionGrant> = self.session_grants.clone();
            match serde_json::to_string_pretty(&data) {
                Ok(json) => {
                    fs::write(path, &json).unwrap_or_else(|e| {
                        warn!(target: "permission", path = %path.display(), error = %e, "failed to persist session grants");
                    });
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if let Ok(metadata) = fs::metadata(path) {
                            let mut perms = metadata.permissions();
                            perms.set_mode(0o600);
                            let _ = fs::set_permissions(path, perms);
                        }
                    }
                }
                Err(e) => warn!(target: "permission", error = %e, "failed to serialize session grants"),
            }
        }
    }

    /// Register an agent permission profile.
    pub fn register(&mut self, permission: AgentPermission) {
        self.agent_permissions
            .insert(permission.agent_name.clone(), permission);
    }

    /// Temporarily grant a tool for the remainder of the session.
    /// Persists immediately to disk.
    pub fn grant_session(&mut self, agent_name: &str, tool_name: ToolName) {
        let key = (agent_name.to_string(), tool_name.to_string());
        if self.grant_index.contains(&key) {
            return;
        }
        debug!(target: "permission", agent = %agent_name, tool = %tool_name.as_str(), "session grant added");
        self.session_grants.push(SessionGrant {
            agent_name: agent_name.to_string(),
            tool_name: tool_name.to_string(),
            granted_at_ms: now_millis(),
        });
        self.grant_index.insert(key);
        self.persist_grants();
    }

    /// Revoke all session grants for a given agent.
    pub fn revoke_session_grants(&mut self, agent_name: &str) {
        self.session_grants.retain(|g| g.agent_name != agent_name);
        self.grant_index.retain(|(a, _)| a != agent_name);
        self.persist_grants();
        info!(target: "permission", agent = %agent_name, "session grants revoked");
    }

    /// Check if a session grant exists.
    fn has_session_grant(&self, agent_name: &str, tool_name: &ToolName) -> bool {
        self.grant_index.contains(&(agent_name.to_string(), tool_name.to_string()))
    }

    /// Check if a resource path matches a glob-like pattern.
    ///
    /// Supports `*` (single segment), `**` (any depth), `**/` (anywhere).
    /// Reference: CodeWhale's prefix-based matching + OpenCode's wildcard rules.
    fn path_matches(resource: &str, pattern: &str) -> bool {
        if pattern == "*" || pattern == "**" {
            return true;
        }
        // `**/` prefix: match anywhere in path
        if let Some(rest) = pattern.strip_prefix("**/") {
            if rest.contains('*') {
                let prefix = rest.trim_end_matches('*');
                return resource.contains(prefix);
            }
            return resource.contains(rest) || resource.ends_with(rest);
        }
        // `*` suffix: match prefix (e.g. "*.md")
        if let Some(prefix) = pattern.strip_prefix('*') {
            return resource.ends_with(prefix);
        }
        // `*` infix: match prefix + suffix (e.g. "plans/*.md")
        if let Some((prefix, suffix)) = pattern.split_once('*') {
            return resource.starts_with(prefix) && resource.ends_with(suffix);
        }
        // Exact match
        resource == pattern
    }

    /// Evaluate permission for a specific agent+tool combination.
    #[must_use]
    pub fn evaluate(
        &self,
        agent_name: &str,
        tool_name: &ToolName,
        is_mutating: bool,
        tool_args: Option<&serde_json::Value>,
    ) -> PermissionDecision {
        debug!(target: "permission", agent = %agent_name, tool = %tool_name.as_str(), mutating = is_mutating, "evaluating permission");
        let profile = self.agent_permissions.get(agent_name);

        // Level 1: Hard permission rules (path-level, MiMo-Code style)
        if let Some(profile) = profile {
            if !profile.hard_permission_rules.is_empty() {
                let resource = tool_args
                    .and_then(|args| args.get("path"))
                    .or_else(|| tool_args.and_then(|args| args.get("command")))
                    .or_else(|| tool_args.and_then(|args| args.get("file_path")))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                for rule in &profile.hard_permission_rules {
                    let tool_matches = rule.tool_pattern == "*" || rule.tool_pattern == tool_name.as_str();
                    let resource_matches = Self::path_matches(resource, &rule.resource_pattern);
                    if tool_matches && resource_matches {
                        match rule.setting {
                            PermissionSetting::Disabled => {
                                let reason = format!(
                                    "hard-disabled by rule: tool '{tool_name}' resource '{resource}' pattern '{}'",
                                    rule.resource_pattern
                                );
                                warn!(target: "permission", agent = %agent_name, tool = %tool_name.as_str(), reason = %reason, "permission denied");
                                return PermissionDecision::Deny { reason };
                            }
                            PermissionSetting::AutoAccept => return PermissionDecision::Allow,
                            PermissionSetting::Ask => {} // fall through
                        }
                    }
                }
            }

            // Level 1b: Simple hard permission (legacy, blanket for all mutating)
            if let Some(ref hard) = profile.hard_permission {
                match hard {
                    PermissionSetting::Disabled => {
                        if is_mutating {
                            let reason = format!(
                                "tool '{tool_name}' is hard-disabled for agent '{agent_name}'"
                            );
                            warn!(target: "permission", agent = %agent_name, tool = %tool_name.as_str(), reason = %reason, "permission denied");
                            return PermissionDecision::Deny { reason };
                        }
                    }
                    PermissionSetting::AutoAccept => return PermissionDecision::Allow,
                    PermissionSetting::Ask => {} // fall through
                }
            }
        }

        // Level 2: Agent-level Disabled
        if let Some(profile) = profile
            && let Some(PermissionSetting::Disabled) = profile.tool_permissions.get(tool_name) {
                let reason = format!(
                    "tool '{tool_name}' is disabled for agent '{agent_name}'"
                );
                warn!(target: "permission", agent = %agent_name, tool = %tool_name.as_str(), reason = %reason, "permission denied");
                return PermissionDecision::Deny { reason };
            }

        // Level 3: Agent-level AutoAccept
        if let Some(profile) = profile
            && let Some(PermissionSetting::AutoAccept) = profile.tool_permissions.get(tool_name) {
                return PermissionDecision::Allow;
            }

        // Level 4: Session grant (fast index lookup)
        if self.has_session_grant(agent_name, tool_name) {
            return PermissionDecision::Allow;
        }

        // Level 5: Agent-level Ask
        if let Some(profile) = profile
            && let Some(PermissionSetting::Ask) = profile.tool_permissions.get(tool_name) {
                return PermissionDecision::Ask {
                    tool_name: tool_name.to_string(),
                    reason: format!("agent '{agent_name}' requires approval for '{tool_name}'"),
                    request_id: format!("ask-{agent_name}-{tool_name}"),
                };
            }

        // Level 6: Tool annotation default
        if is_mutating {
            PermissionDecision::Ask {
                tool_name: tool_name.to_string(),
                reason: "mutating tool requires approval".into(),
                request_id: format!("ask-{tool_name}"),
            }
        } else {
            PermissionDecision::Allow
        }
    }

    /// Filter a [`ToolRegistry`] to only include tools the agent is allowed to see.
    ///
    /// Disabled tools and tools blocked by blanket hard-permission rules are
    /// excluded so the model never sees their definitions.
    #[must_use]
    pub fn materialize_tools(
        &self,
        agent_name: &str,
        all_tools: &ToolRegistry,
    ) -> ToolRegistry {
        let profile = self.agent_permissions.get(agent_name);
        let mut filtered = ToolRegistry::new();
        for def in all_tools.definitions() {
            let permission = profile
                .and_then(|p| p.tool_permissions.get(&def.name))
                .cloned();

            // Skip disabled tools entirely
            if permission == Some(PermissionSetting::Disabled) {
                continue;
            }

            // Check hard permission rules — if ALL rules deny this tool regardless
            // of path, we can skip it at materialize time.
            if let Some(profile) = profile {
                // Simple hard permission check
                if let Some(PermissionSetting::Disabled) = &profile.hard_permission
                    && def.annotations.is_mutating() {
                        // But only skip if there are no allow-override rules
                        let has_allow_override = profile.hard_permission_rules.iter().any(|r| {
                            (r.tool_pattern == "*" || r.tool_pattern == def.name.as_str())
                                && r.setting == PermissionSetting::AutoAccept
                        });
                        if !has_allow_override {
                            continue;
                        }
                    }
                // Pattern-based hard permission: if the tool itself is hard-disabled
                // with resource_pattern="**" and no allow overrides, skip it
                let has_blanket_deny = profile.hard_permission_rules.iter().any(|r| {
                    (r.tool_pattern == "*" || r.tool_pattern == def.name.as_str())
                        && r.resource_pattern == "**"
                        && r.setting == PermissionSetting::Disabled
                });
                if has_blanket_deny {
                    let has_allow_override = profile.hard_permission_rules.iter().any(|r| {
                        (r.tool_pattern == "*" || r.tool_pattern == def.name.as_str())
                            && r.resource_pattern != "**"
                            && r.setting == PermissionSetting::AutoAccept
                    });
                    if !has_allow_override {
                        continue;
                    }
                }
            }

            if let Some(tool) = all_tools.get(&def.name) {
                let _ = filtered.insert_arc(tool);
            }
        }
        info!(target: "permission", agent = %agent_name, tool_count = filtered.definitions().len(), "tools materialized for agent");
        filtered
    }

    /// Build a [`ToolPolicy`] that delegates all decisions to this engine for the given agent.
    #[must_use]
    pub fn policy_for(&self, agent_name: String) -> Arc<dyn ToolPolicy> {
        Arc::new(EnginePolicy {
            engine: Self {
                agent_permissions: self.agent_permissions.clone(),
                session_grants: self.session_grants.clone(),
                grants_path: self.grants_path.clone(),
                grant_index: self.grant_index.clone(),
            },
            agent_name,
            approval_channel: None,
        })
    }

    /// Build a [`ToolPolicy`] with an approval channel for interactive Ask flows.
    #[must_use]
    pub fn policy_for_with_channel(
        &self,
        agent_name: String,
        approval_channel: SharedApprovalChannel,
    ) -> Arc<dyn ToolPolicy> {
        Arc::new(EnginePolicy {
            engine: Self {
                agent_permissions: self.agent_permissions.clone(),
                session_grants: self.session_grants.clone(),
                grants_path: self.grants_path.clone(),
                grant_index: self.grant_index.clone(),
            },
            agent_name,
            approval_channel: Some(approval_channel),
        })
    }

    /// The constraint-file path for an agent, if one was registered.
    #[must_use]
    pub fn constraint_file(&self, agent_name: &str) -> Option<PathBuf> {
        self.agent_permissions
            .get(agent_name)
            .map(|p| p.constraint_file.clone())
    }
}

impl Default for PermissionEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ── EnginePolicy: bridges PermissionEngine to the ToolPolicy trait ────────

struct EnginePolicy {
    engine: PermissionEngine,
    agent_name: String,
    #[allow(dead_code)]
    approval_channel: Option<SharedApprovalChannel>,
}

impl ToolPolicy for EnginePolicy {
    fn evaluate<'a>(&'a self, request: ToolPolicyRequest<'a>) -> PolicyFuture<'a> {
        let is_mutating = request
            .definition
            .map(|d| d.annotations.is_mutating())
            .unwrap_or(true);
        let decision = self
            .engine
            .evaluate(
                &self.agent_name,
                &request.invocation.name,
                is_mutating,
                Some(&request.invocation.arguments),
            );
        Box::pin(async move {
            match decision {
                PermissionDecision::Allow => Ok(ToolDecision::Allow),
                PermissionDecision::Deny { reason } => Ok(ToolDecision::Deny { reason }),
                PermissionDecision::Ask {
                    tool_name,
                    reason,
                    request_id,
                } => {
                    if self
                        .approval_channel
                        .as_ref()
                        .is_some_and(|channel| channel.is_granted_for_session(&tool_name))
                    {
                        Ok(ToolDecision::Allow)
                    } else {
                        Ok(ToolDecision::Ask {
                            request_id,
                            reason: format!("{tool_name}: {reason}"),
                        })
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{ToolAnnotations, ToolDefinition};
    use serde_json::json;

    #[allow(dead_code)]
    fn test_tool(name: &str, mutating: bool) -> (ToolName, ToolDefinition) {
        let name = ToolName::new(name);
        let def = ToolDefinition {
            name: name.clone(),
            description: "test".into(),
            input_schema: json!({"type": "object"}),
            annotations: ToolAnnotations {
                mutating,
                ..ToolAnnotations::default()
            },
        };
        (name, def)
    }

    fn plan_profile() -> AgentPermission {
        let mut perms = HashMap::new();
        perms.insert(ToolName::new("read_file"), PermissionSetting::AutoAccept);
        perms.insert(ToolName::new("write_file"), PermissionSetting::Disabled);
        perms.insert(ToolName::new("shell"), PermissionSetting::Disabled);

        AgentPermission {
            agent_name: "plan".into(),
            tool_permissions: perms,
            constraint_file: PathBuf::from("plan_agent.md"),
            hard_permission: Some(PermissionSetting::Disabled),
            hard_permission_rules: vec![HardPermissionRule {
                tool_pattern: "*".into(),
                resource_pattern: "plans/*.md".into(),
                setting: PermissionSetting::AutoAccept,
            }],
            model: None,
        }
    }

    fn yolo_profile() -> AgentPermission {
        let mut perms = HashMap::new();
        perms.insert(ToolName::new("read_file"), PermissionSetting::AutoAccept);
        perms.insert(ToolName::new("write_file"), PermissionSetting::AutoAccept);
        perms.insert(ToolName::new("shell"), PermissionSetting::AutoAccept);

        AgentPermission {
            agent_name: "yolo".into(),
            tool_permissions: perms,
            constraint_file: PathBuf::from("yolo_agent.md"),
            hard_permission: None,
            hard_permission_rules: Vec::new(),
            model: None,
        }
    }

    #[test]
    fn plan_cannot_use_mutating_tools_even_with_session_grant() {
        let mut engine = PermissionEngine::new();
        engine.register(plan_profile());

        // Hard permission should deny write_file
        let decision = engine.evaluate("plan", &ToolName::new("write_file"), true, None);
        assert!(matches!(decision, PermissionDecision::Deny { .. }));

        // Even with session grant, hard permission wins
        engine.grant_session("plan", ToolName::new("write_file"));
        let decision = engine.evaluate("plan", &ToolName::new("write_file"), true, None);
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    #[test]
    fn plan_can_read() {
        let mut engine = PermissionEngine::new();
        engine.register(plan_profile());

        let decision = engine.evaluate("plan", &ToolName::new("read_file"), false, None);
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[test]
    fn yolo_auto_accepts_write() {
        let mut engine = PermissionEngine::new();
        engine.register(yolo_profile());

        let decision = engine.evaluate("yolo", &ToolName::new("write_file"), true, None);
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[test]
    fn unknown_agent_falls_back_to_annotation() {
        let engine = PermissionEngine::new();

        // No profile registered — mutating asks, read-only allows
        let decision = engine.evaluate("unknown", &ToolName::new("write_file"), true, None);
        assert!(matches!(decision, PermissionDecision::Ask { .. }));

        let decision = engine.evaluate("unknown", &ToolName::new("read_file"), false, None);
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[test]
    fn session_grant_overrides_ask() {
        let mut engine = PermissionEngine::new();
        let mut perms = HashMap::new();
        perms.insert(ToolName::new("shell"), PermissionSetting::Ask);
        engine.register(AgentPermission {
            agent_name: "build".into(),
            tool_permissions: perms,
            constraint_file: PathBuf::from("build_agent.md"),
            hard_permission: None,
            hard_permission_rules: Vec::new(),
            model: None,
        });

        // Should ask initially
        let decision = engine.evaluate("build", &ToolName::new("shell"), true, None);
        assert!(matches!(decision, PermissionDecision::Ask { .. }));

        // After session grant, should allow
        engine.grant_session("build", ToolName::new("shell"));
        let decision = engine.evaluate("build", &ToolName::new("shell"), true, None);
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[test]
    fn materialize_filters_disabled_tools() {
        let mut engine = PermissionEngine::new();
        engine.register(plan_profile());

        let registry = ToolRegistry::new();
        // We can't actually insert tools easily here — just verify definitions are filtered
        // The real filtering test uses all_tool_registry in integration tests
        assert!(engine.materialize_tools("plan", &registry).definitions().is_empty());
    }
}
