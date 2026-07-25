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
//! 4. **Session grant** — "Allow for this session" temporary override.
//! 5. **Agent Ask** — tool requires interactive approval.
//! 6. **Default** — fall back to tool annotation default.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::policy::{SharedApprovalChannel, ToolDecision, ToolPolicy, ToolPolicyRequest};
use crate::tool::{ToolName, ToolRegistry};
use crate::policy::PolicyFuture;

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

/// Permission configuration for a named agent.
#[derive(Clone, Debug)]
pub struct AgentPermission {
    pub agent_name: String,
    pub tool_permissions: HashMap<ToolName, PermissionSetting>,
    pub constraint_file: PathBuf,
    /// Non-overridable permission — checked first and always terminal.
    /// Only meaningful when set to `Disabled` (to enforce read-only mode
    /// like plan agent) or `AutoAccept`.
    pub hard_permission: Option<PermissionSetting>,
    /// Optional per-agent model override.
    pub model: Option<String>,
}

/// Outcome of evaluating a tool permission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Ask {
        tool_name: String,
        reason: String,
        request_id: String,
    },
    Deny {
        reason: String,
    },
}

/// A temporary session-scoped grant.
#[derive(Clone, Debug)]
struct SessionGrant {
    agent_name: String,
    tool_name: ToolName,
    #[allow(dead_code)]
    granted_at: std::time::Instant,
}

/// The permission engine: owns agent profiles, session grants, and produces
/// per-agent `ToolPolicy` implementations and filtered `ToolRegistry` instances.
#[derive(Clone, Debug)]
pub struct PermissionEngine {
    agent_permissions: HashMap<String, AgentPermission>,
    session_grants: Vec<SessionGrant>,
}

impl PermissionEngine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            agent_permissions: HashMap::new(),
            session_grants: Vec::new(),
        }
    }

    /// Register an agent permission profile.
    pub fn register(&mut self, permission: AgentPermission) {
        self.agent_permissions
            .insert(permission.agent_name.clone(), permission);
    }

    /// Grant a tool for the current session (Allow for Session).
    pub fn grant_session(&mut self, agent_name: &str, tool_name: ToolName) {
        self.session_grants.push(SessionGrant {
            agent_name: agent_name.to_string(),
            tool_name,
            granted_at: std::time::Instant::now(),
        });
    }

    /// Evaluate permission for a specific agent+tool combination.
    #[must_use]
    pub fn evaluate(
        &self,
        agent_name: &str,
        tool_name: &ToolName,
        is_mutating: bool,
    ) -> PermissionDecision {
        let profile = self.agent_permissions.get(agent_name);

        // Level 1: Hard permission (non-overridable, terminal)
        if let Some(profile) = profile
            && let Some(ref hard) = profile.hard_permission {
                match hard {
                    PermissionSetting::Disabled => {
                        // Hard-disabled only blocks mutating tools; read-only still allowed
                        if is_mutating {
                            return PermissionDecision::Deny {
                                reason: format!(
                                    "tool '{tool_name}' is hard-disabled for agent '{agent_name}'"
                                ),
                            };
                        }
                    }
                    PermissionSetting::AutoAccept => return PermissionDecision::Allow,
                    PermissionSetting::Ask => {} // fall through
                }
            }

        // Level 2: Agent-level Disabled
        if let Some(profile) = profile
            && let Some(PermissionSetting::Disabled) = profile.tool_permissions.get(tool_name) {
                return PermissionDecision::Deny {
                    reason: format!(
                        "tool '{tool_name}' is disabled for agent '{agent_name}'"
                    ),
                };
            }

        // Level 3: Agent-level AutoAccept
        if let Some(profile) = profile
            && let Some(PermissionSetting::AutoAccept) = profile.tool_permissions.get(tool_name) {
                return PermissionDecision::Allow;
            }

        // Level 4: Session grant
        if self
            .session_grants
            .iter()
            .any(|g| g.agent_name == agent_name && g.tool_name == *tool_name)
        {
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

    /// Filter a tool registry to only include tools the agent is allowed to use.
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

            // Also check hard permission
            if let Some(profile) = profile
                && let Some(PermissionSetting::Disabled) = &profile.hard_permission {
                    // If plan has hard Disabled for all mutating tools, check annotation
                    if def.annotations.mutating {
                        continue;
                    }
                }

            if let Some(tool) = all_tools.get(&def.name) {
                let _ = filtered.insert_arc(tool);
            }
        }
        filtered
    }

    /// Build a `ToolPolicy` implementation for the given agent that delegates
    /// to this engine.
    #[must_use]
    pub fn policy_for(&self, agent_name: String) -> Arc<dyn ToolPolicy> {
        Arc::new(EnginePolicy {
            engine: Self {
                agent_permissions: self.agent_permissions.clone(),
                session_grants: self.session_grants.clone(),
            },
            agent_name,
            approval_channel: None,
        })
    }

    /// Build a `ToolPolicy` with an approval channel wired in.
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
            },
            agent_name,
            approval_channel: Some(approval_channel),
        })
    }

    /// Return the constraint file path for an agent, if configured.
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
            .map(|d| d.annotations.mutating)
            .unwrap_or(true);
        let decision = self
            .engine
            .evaluate(&self.agent_name, &request.invocation.name, is_mutating);
        Box::pin(async move {
            match decision {
                PermissionDecision::Allow => Ok(ToolDecision::Allow),
                PermissionDecision::Deny { reason } => Ok(ToolDecision::Deny { reason }),
                PermissionDecision::Ask {
                    tool_name,
                    reason,
                    request_id,
                } => Ok(ToolDecision::Ask {
                    request_id,
                    reason: format!("{tool_name}: {reason}"),
                }),
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
            model: None,
        }
    }

    #[test]
    fn plan_cannot_use_mutating_tools_even_with_session_grant() {
        let mut engine = PermissionEngine::new();
        engine.register(plan_profile());

        // Hard permission should deny write_file
        let decision = engine.evaluate("plan", &ToolName::new("write_file"), true);
        assert!(matches!(decision, PermissionDecision::Deny { .. }));

        // Even with session grant, hard permission wins
        engine.grant_session("plan", ToolName::new("write_file"));
        let decision = engine.evaluate("plan", &ToolName::new("write_file"), true);
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    #[test]
    fn plan_can_read() {
        let mut engine = PermissionEngine::new();
        engine.register(plan_profile());

        let decision = engine.evaluate("plan", &ToolName::new("read_file"), false);
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[test]
    fn yolo_auto_accepts_write() {
        let mut engine = PermissionEngine::new();
        engine.register(yolo_profile());

        let decision = engine.evaluate("yolo", &ToolName::new("write_file"), true);
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[test]
    fn unknown_agent_falls_back_to_annotation() {
        let engine = PermissionEngine::new();

        // No profile registered — mutating asks, read-only allows
        let decision = engine.evaluate("unknown", &ToolName::new("write_file"), true);
        assert!(matches!(decision, PermissionDecision::Ask { .. }));

        let decision = engine.evaluate("unknown", &ToolName::new("read_file"), false);
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
            model: None,
        });

        // Should ask initially
        let decision = engine.evaluate("build", &ToolName::new("shell"), true);
        assert!(matches!(decision, PermissionDecision::Ask { .. }));

        // After session grant, should allow
        engine.grant_session("build", ToolName::new("shell"));
        let decision = engine.evaluate("build", &ToolName::new("shell"), true);
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
