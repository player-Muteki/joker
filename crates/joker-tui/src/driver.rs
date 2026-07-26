use std::{path::PathBuf, sync::Arc};

use joker::{
    Agent, AgentPermission, ModelResponseEvent, Observer, ObserverFuture, PermissionEngine,
    PermissionPolicy, PermissionRule, PermissionSetting, RunRequest, ScriptedModel, ScriptedStep,
    SharedApprovalChannel, StopReason, ToolAnnotations, ToolDefinition, ToolDecision, ToolFn,
    ToolFuture, ToolInvocation, ToolName, ToolOutput, ToolPolicy, ToolPolicyRequest,
    builtin_agent_profiles, PolicyFuture, RulePattern, PrefixedContextBuilder, assemble_system_prompt,
};
use joker_config::{ProviderSelection, RuntimeConfig};
use joker_mcp::connect_and_discover;
use joker_tools::all_tool_registry;
use serde_json::json;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{TuiError, event::UiEvent};

#[derive(Clone)]
pub struct ChannelObserver {
    tx: tokio::sync::mpsc::UnboundedSender<UiEvent>,
}

impl ChannelObserver {
    #[must_use]
    pub fn new(tx: tokio::sync::mpsc::UnboundedSender<UiEvent>) -> Self {
        Self { tx }
    }
}

impl Observer for ChannelObserver {
    fn observe(&self, event: joker::Event) -> ObserverFuture<'_> {
        let tx = self.tx.clone();
        Box::pin(async move {
            let _ = tx.send(UiEvent::Agent(event));
            Ok(())
        })
    }
}

#[derive(Clone)]
pub struct AgentDriver {
    runtime_config: RuntimeConfig,
    workspace: PathBuf,
    agents_dir: PathBuf,
    compact_pending: bool,
    permission_engine: PermissionEngine,
    active_agent: String,
    mcp_tools: Arc<std::sync::Mutex<Vec<Arc<dyn joker::Tool>>>>,
}

impl std::fmt::Debug for AgentDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentDriver")
            .field("runtime_config", &self.runtime_config)
            .field("workspace", &self.workspace)
            .field("agents_dir", &self.agents_dir)
            .field("compact_pending", &self.compact_pending)
            .field("active_agent", &self.active_agent)
            .field("mcp_tools", &self.mcp_tools.lock().unwrap().len())
            .finish()
    }
}

impl AgentDriver {
    #[must_use]
    pub fn new(runtime_config: RuntimeConfig, workspace: impl Into<PathBuf>) -> Self {
        let workspace: PathBuf = workspace.into();
        Self::new_with_agents_dir(
            runtime_config,
            workspace.clone(),
            workspace.join(".joker").join("agents"),
        )
    }

    #[must_use]
    pub fn new_with_agents_dir(
        runtime_config: RuntimeConfig,
        workspace: impl Into<PathBuf>,
        agents_dir: impl Into<PathBuf>,
    ) -> Self {
        let workspace: PathBuf = workspace.into();
        let agents_dir: PathBuf = agents_dir.into();
        // Write built-in constraint files if they don't exist
        let _ = std::fs::create_dir_all(&agents_dir);
        for name in &["plan", "build", "yolo"] {
            let path = agents_dir.join(format!("{name}_agent.md"));
            if !path.exists() {
                let content = joker::builtin_constraint_file_content(name);
                if !content.is_empty() {
                    let _ = std::fs::write(&path, content);
                }
            }
        }

        let mut engine = PermissionEngine::new();
        // Register built-in agent profiles (plan, build, yolo)
        let builtins = builtin_agent_profiles(&agents_dir);
        for profile in builtins {
            engine.register(profile);
        }
        // Register custom agent profiles from config
        for (name, agent_cfg) in &runtime_config.to_file_config().agent {
            if !["plan", "build", "yolo"].contains(&name.as_str()) {
                let permission = agent_permission_from_config(name, agent_cfg, &agents_dir);
                engine.register(permission);
            }
        }

        Self {
            runtime_config,
            workspace,
            agents_dir,
            compact_pending: false,
            permission_engine: engine,
            active_agent: "build".into(),
            mcp_tools: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub fn set_runtime_config(&mut self, runtime_config: RuntimeConfig) {
        self.runtime_config = runtime_config;
    }

    #[must_use]
    pub fn active_agent(&self) -> &str {
        &self.active_agent
    }

    pub fn set_active_agent(&mut self, agent_name: String) {
        self.active_agent = agent_name;
    }

    #[must_use]
    pub fn permission_engine(&self) -> &PermissionEngine {
        &self.permission_engine
    }

    #[must_use]
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    #[must_use]
    pub fn agents_dir(&self) -> &PathBuf {
        &self.agents_dir
    }

    pub fn permission_engine_mut(&mut self) -> &mut PermissionEngine {
        &mut self.permission_engine
    }

    pub fn spawn_run(
        &self,
        prompt: String,
        cancellation_token: CancellationToken,
        tx: tokio::sync::mpsc::UnboundedSender<UiEvent>,
        approval_channel: SharedApprovalChannel,
    ) -> Result<JoinHandle<()>, TuiError> {
        let agent = self.build_agent(tx.clone(), approval_channel.clone())?;
        Ok(tokio::spawn(async move {
            let request = RunRequest::new(prompt)
                .with_cancellation_token(cancellation_token)
                .with_approval_channel(approval_channel);
            let result = agent
                .run(request)
                .await
                .map_err(|error| error.to_string());
            let _ = tx.send(UiEvent::RunCompleted(result));
        }))
    }

    pub fn spawn_run_with_conversation(
        &self,
        conversation: joker::Conversation,
        cancellation_token: CancellationToken,
        tx: tokio::sync::mpsc::UnboundedSender<UiEvent>,
        approval_channel: SharedApprovalChannel,
    ) -> Result<JoinHandle<()>, TuiError> {
        let agent = self.build_agent(tx.clone(), approval_channel.clone())?;
        Ok(tokio::spawn(async move {
            let request = RunRequest::with_conversation(conversation)
                .with_cancellation_token(cancellation_token)
                .with_approval_channel(approval_channel);
            let result = agent
                .run(request)
                .await
                .map_err(|error| error.to_string());
            let _ = tx.send(UiEvent::RunCompleted(result));
        }))
    }

    pub fn set_compact_pending(&mut self, pending: bool) {
        self.compact_pending = pending;
    }

    /// Connect to configured MCP servers and discover their tools.
    /// Call this once after construction, before spawning agent runs.
    pub async fn init_mcp_servers(&self) {
        let config = self.runtime_config.to_file_config();
        if config.mcp_servers.is_empty() {
            return;
        }

        let mut tools: Vec<Arc<dyn joker::Tool>> = Vec::new();
        for server_cfg in config.mcp_servers.values() {
            if let Some(command) = &server_cfg.command {
                let mcp_cfg = joker_mcp::McpToolConfig {
                    command: command.clone(),
                    args: server_cfg.args.clone(),
                };
                match connect_and_discover(&mcp_cfg).await {
                    Ok((_client, adapters)) => {
                        for adapter in adapters {
                            tools.push(Arc::new(adapter));
                        }
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to connect to MCP server {}: {e}", command);
                    }
                }
            }
        }

        if let Ok(mut guard) = self.mcp_tools.lock() {
            *guard = tools;
        }
    }
    fn build_agent(
        &self,
        tx: tokio::sync::mpsc::UnboundedSender<UiEvent>,
        approval_channel: SharedApprovalChannel,
    ) -> Result<Agent, TuiError> {
        let model = self.build_model()?;
        let observer = Arc::new(ChannelObserver::new(tx.clone()));
        let mut agent = Agent::new(model)
            .with_observer(observer.clone())
            .with_approval_channel(approval_channel.clone());

        // Use permission engine to filter tools for the active agent
        let mut registry = all_tool_registry(&self.workspace)
            .map_err(|error| TuiError::Agent(error.to_string()))?;

        // Add MCP tools
        if let Ok(guard) = self.mcp_tools.lock() {
            for tool in guard.iter() {
                let _ = registry.insert_arc(tool.clone());
            }
        }

        let filtered = self
            .permission_engine
            .materialize_tools(&self.active_agent, &registry);
        agent = agent.with_tools(Arc::new(filtered));

        // Compose: safety policy (dangerous commands) → engine policy (agent permissions)
        let safety_policy = PermissionPolicy::new()
            .with_rules(vec![
                PermissionRule::new(
                    RulePattern::CommandPrefix("rm -rf".into()),
                    ToolDecision::Deny {
                        reason: "dangerous command".into(),
                    },
                ),
                PermissionRule::new(
                    RulePattern::CommandPrefix("sudo".into()),
                    ToolDecision::Deny {
                        reason: "sudo not allowed".into(),
                    },
                ),
            ]);
        let engine_policy = self.permission_engine.policy_for_with_channel(
            self.active_agent.clone(),
            approval_channel,
        );
        let policy = Arc::new(ChainPolicy {
            first: Arc::new(safety_policy),
            second: engine_policy,
        });
        agent = agent.with_policy(policy);

        // Wrap context builder with system prompt from agent profile
        let system_prompt = assemble_system_prompt(&self.active_agent, None, None);
        let inner: Box<dyn joker::ContextBuilder> = if self.compact_pending {
            Box::new(
                joker::CompactingContextBuilder::new(Box::new(joker::PassthroughContextBuilder))
                    .with_observer(observer),
            )
        } else {
            Box::new(joker::PassthroughContextBuilder)
        };
        let context_builder: Arc<dyn joker::ContextBuilder> =
            Arc::new(PrefixedContextBuilder::new(system_prompt, inner));
        agent = agent.with_context_builder(context_builder);

        Ok(agent)
    }

    fn build_model(&self) -> Result<Arc<dyn joker::Model>, TuiError> {
        match &self.runtime_config.provider {
            ProviderSelection::Scripted { .. } => {
                Ok(Arc::new(ScriptedModel::new(self.scripted_steps()))
                    as Arc<dyn joker::Model>)
            }
            ProviderSelection::Route(route) => {
                let model = if route.default_model.is_empty() {
                    "model"
                } else {
                    &route.default_model
                };
                route.build_model_for(model)
                    .map_err(TuiError::Agent)
            }
        }
    }

    fn scripted_steps(&self) -> Vec<ScriptedStep> {
        vec![ScriptedStep::Events(streaming_text_events(
            &self.runtime_config.scripted_response,
        ))]
    }
}

fn streaming_text_events(text: &str) -> Vec<ModelResponseEvent> {
    let mut events = text
        .split_inclusive(char::is_whitespace)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| ModelResponseEvent::TextDelta(chunk.to_string()))
        .collect::<Vec<_>>();

    if events.is_empty() && !text.is_empty() {
        events.push(ModelResponseEvent::TextDelta(text.to_string()));
    }

    events.push(ModelResponseEvent::Finished {
        stop_reason: StopReason::Stop,
        usage: joker::Usage::default(),
    });
    events
}

/// Compose two `ToolPolicy` impls: first's Deny takes priority, then delegates to second.
/// Also downgrades Allow→Ask for shell commands with redirect/pipeline operators.
struct ChainPolicy {
    first: Arc<dyn ToolPolicy>,
    second: Arc<dyn ToolPolicy>,
}

/// Shell operators that trigger `Allow` → `Ask` downgrade.
const SHELL_REDIRECT_PATTERNS: &[&str] = &["|", ">", "$(", "`"];

impl ChainPolicy {
    fn has_redirect_operators(command: &str) -> bool {
        SHELL_REDIRECT_PATTERNS
            .iter()
            .any(|op| command.contains(op))
    }
}

impl ToolPolicy for ChainPolicy {
    fn evaluate<'a>(&'a self, request: ToolPolicyRequest<'a>) -> PolicyFuture<'a> {
        let request2 = request.clone();
        Box::pin(async move {
            let first_decision = self.first.evaluate(request).await?;
            match first_decision {
                ToolDecision::Deny { .. } => Ok(first_decision),
                _ => {
                    // Check for shell redirect downgrade
                    if first_decision == ToolDecision::Allow
                        && request2.invocation.name.as_str() == "shell"
                        && let Some(cmd) = request2
                            .invocation
                            .arguments
                            .get("command")
                            .and_then(|v| v.as_str())
                            && Self::has_redirect_operators(cmd) {
                                return Ok(ToolDecision::Ask {
                                    request_id: "redirect-downgrade".into(),
                                    reason: format!(
                                        "shell command contains redirect/pipeline operators ({:?})",
                                        SHELL_REDIRECT_PATTERNS
                                    ),
                                });
                            }
                    self.second.evaluate(request2).await
                }
            }
        })
    }
}

/// Convert an `AgentProfileConfig` from the config layer into an `AgentPermission`.
fn agent_permission_from_config(
    name: &str,
    cfg: &joker_config::AgentProfileConfig,
    agents_dir: &std::path::Path,
) -> AgentPermission {
    use std::collections::HashMap;
    let mut perms = HashMap::new();
    for (tool_name, tool_cfg) in &cfg.tools {
        let setting = match tool_cfg.permission.as_deref() {
            Some("auto-accept" | "auto_accept" | "auto") => PermissionSetting::AutoAccept,
            Some("ask") => PermissionSetting::Ask,
            Some("disabled" | "disable" | "deny" | "none") => PermissionSetting::Disabled,
            _ => {
                // If enabled is explicitly false, disable; otherwise default to Ask
                if tool_cfg.enabled == Some(false) {
                    PermissionSetting::Disabled
                } else {
                    PermissionSetting::Ask
                }
            }
        };
        perms.insert(ToolName::new(tool_name), setting);
    }
    AgentPermission {
        agent_name: name.to_string(),
        tool_permissions: perms,
        constraint_file: agents_dir.join(format!("{name}_agent.md")),
        hard_permission: None,
        hard_permission_rules: Vec::new(),
        model: None,
    }
}

#[allow(dead_code)]
fn make_echo_tool() -> ToolFn<fn(ToolInvocation) -> ToolFuture<'static>> {
    fn echo(invocation: ToolInvocation) -> ToolFuture<'static> {
        Box::pin(async move {
            Ok(ToolOutput::new(json!({
                "echo": invocation.arguments.get("text").and_then(|value| value.as_str()).unwrap_or("")
            })))
        })
    }

    ToolFn::new(
        ToolDefinition {
            name: ToolName::new("echo"),
            description: "Returns the submitted prompt text.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                },
                "required": ["text"]
            }),
            annotations: ToolAnnotations::default(),
        },
        echo as fn(ToolInvocation) -> ToolFuture<'static>,
    )
}
