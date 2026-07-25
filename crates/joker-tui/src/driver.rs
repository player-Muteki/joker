use std::{path::PathBuf, sync::Arc};

use joker::{
    Agent, AgentPermission, ModelResponseEvent, Observer, ObserverFuture, PermissionEngine,
    PermissionPolicy, PermissionRule, PermissionSetting, RunRequest, ScriptedModel, ScriptedStep,
    SharedApprovalChannel, StopReason, ToolAnnotations, ToolDefinition, ToolDecision, ToolFn,
    ToolFuture, ToolInvocation, ToolName, ToolOutput, ToolPolicy, ToolPolicyRequest,
    builtin_agent_profiles, PolicyFuture, RulePattern, SummaryContextBuilder,
};
use joker_config::{ProviderSelection, RuntimeConfig};
use joker_provider::{anthropic, google};
use joker_provider::OpenAiCompatibleModel;
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

#[derive(Clone, Debug)]
pub struct AgentDriver {
    runtime_config: RuntimeConfig,
    workspace: PathBuf,
    compact_pending: bool,
    permission_engine: PermissionEngine,
    active_agent: String,
}

impl AgentDriver {
    #[must_use]
    pub fn new(runtime_config: RuntimeConfig, workspace: impl Into<PathBuf>) -> Self {
        let workspace: PathBuf = workspace.into();
        let agents_dir = workspace.join(".joker").join("agents");

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
            compact_pending: false,
            permission_engine: engine,
            active_agent: "build".into(),
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

    pub fn set_compact_pending(&mut self, pending: bool) {
        self.compact_pending = pending;
    }
    fn build_agent(
        &self,
        tx: tokio::sync::mpsc::UnboundedSender<UiEvent>,
        approval_channel: SharedApprovalChannel,
    ) -> Result<Agent, TuiError> {
        let model = self.build_model()?;
        let mut agent = Agent::new(model)
            .with_observer(Arc::new(ChannelObserver::new(tx)))
            .with_approval_channel(approval_channel.clone());

        // Use permission engine to filter tools for the active agent
        let registry = all_tool_registry(&self.workspace)
            .map_err(|error| TuiError::Agent(error.to_string()))?;
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

        // Wrap context builder with SummaryContextBuilder when compact is pending
        let context_builder: Arc<dyn joker::ContextBuilder> = if self.compact_pending {
            Arc::new(SummaryContextBuilder::new(
                20,
                Box::new(joker::PassthroughContextBuilder),
            ))
        } else {
            Arc::new(joker::PassthroughContextBuilder)
        };
        agent = agent.with_context_builder(context_builder);

        Ok(agent)
    }

    fn build_model(&self) -> Result<Arc<dyn joker::Model>, TuiError> {
        match &self.runtime_config.provider {
            ProviderSelection::Scripted { .. } => {
                Ok(Arc::new(ScriptedModel::new(self.scripted_steps()))
                    as Arc<dyn joker::Model>)
            }
            ProviderSelection::OpenAiCompatible(config) => Ok(Arc::new(
                OpenAiCompatibleModel::new(config.clone())
                    .map_err(|error| TuiError::Agent(error.to_string()))?,
            )
                as Arc<dyn joker::Model>),
            ProviderSelection::Anthropic { model, api_key } => {
                let cfg = anthropic::AnthropicConfig {
                    base_url: anthropic::DEFAULT_BASE_URL.into(),
                    model: model.clone(),
                    api_key: api_key.clone().unwrap_or_default(),
                };
                Ok(Arc::new(
                    anthropic::AnthropicModel::new(cfg)
                        .map_err(|e| TuiError::Agent(e.to_string()))?,
                )
                    as Arc<dyn joker::Model>)
            }
            ProviderSelection::Google { model, api_key } => {
                let cfg = google::GoogleConfig {
                    base_url: google::DEFAULT_BASE_URL.into(),
                    model: model.clone(),
                    api_key: api_key.clone().unwrap_or_default(),
                };
                Ok(Arc::new(
                    google::GoogleModel::new(cfg)
                        .map_err(|e| TuiError::Agent(e.to_string()))?,
                )
                    as Arc<dyn joker::Model>)
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
struct ChainPolicy {
    first: Arc<dyn ToolPolicy>,
    second: Arc<dyn ToolPolicy>,
}

impl ToolPolicy for ChainPolicy {
    fn evaluate<'a>(&'a self, request: ToolPolicyRequest<'a>) -> PolicyFuture<'a> {
        let request2 = request.clone();
        Box::pin(async move {
            let first_decision = self.first.evaluate(request).await?;
            match first_decision {
                ToolDecision::Deny { .. } => Ok(first_decision),
                _ => self.second.evaluate(request2).await,
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
