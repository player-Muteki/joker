//! Agent lifecycle driver that bridges the TUI event loop to the
//! [`joker::Agent`] runtime.
//!
//! [`AgentDriver`] owns the [`joker::PermissionEngine`], builds [`joker::Agent`]
//! instances with the correct model, tools, and policy chain, and spawns
//! async agent runs.  [`ChannelObserver`] forwards [`joker::Event`] values
//! into an mpsc channel as [`crate::event::UiEvent::Agent`].

use std::{path::PathBuf, sync::Arc};
use tracing::*;

use joker::{
    Agent, AgentProfileCatalog, AgentProfileSpec, AgentRuntime, AgentRuntimeHandle,
    AgentToolPermissionSpec, ModelResponseEvent, NoopObserver, Observer, ObserverFuture,
    PermissionEngine, PermissionPolicy, PermissionRule, PolicyFuture, PrefixedContextBuilder,
    RulePattern, RunRequest, ScriptedModel, ScriptedStep, SharedApprovalChannel, StopReason,
    ToolAnnotations, ToolDecision, ToolDefinition, ToolFn, ToolFuture, ToolInvocation, ToolName,
    ToolOutput, ToolPolicy, ToolPolicyRequest,
};
use joker_config::{ProviderSelection, RuntimeConfig};
use joker_mcp::connect_and_discover;
use joker_tools::all_tool_registry;
use serde_json::json;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{TuiError, event::UiEvent};

/// An [`Observer`] that forwards [`joker::Event`] values into an mpsc channel.
#[derive(Clone)]
pub struct ChannelObserver {
    tx: tokio::sync::mpsc::UnboundedSender<UiEvent>,
}

impl ChannelObserver {
    /// Create a new `ChannelObserver` that sends events through `tx`.
    #[must_use]
    pub fn new(tx: tokio::sync::mpsc::UnboundedSender<UiEvent>) -> Self {
        Self { tx }
    }
}

impl Observer for ChannelObserver {
    fn observe(&self, event: joker::Event) -> ObserverFuture<'_> {
        let tx = self.tx.clone();
        Box::pin(async move {
            trace!(target: "tui.driver", ?event, "agent event");
            let _ = tx.send(UiEvent::Agent(event));
            Ok(())
        })
    }
}

/// Drives agent runs: builds [`Agent`] instances, manages permissions, and spawns async tasks.
#[derive(Clone)]
pub struct AgentDriver {
    runtime_config: RuntimeConfig,
    credential_store: joker::CredentialStore,
    workspace: PathBuf,
    agents_dir: PathBuf,
    compact_pending: bool,
    agent_catalog: AgentProfileCatalog,
    permission_engine: PermissionEngine,
    active_agent: String,
    mcp_tools: Arc<std::sync::Mutex<Vec<Arc<dyn joker::Tool>>>>,
}

/// Result returned when spawning an interactive agent run.
pub struct SpawnedAgentRun {
    /// Handle for controlling the active runtime via Op messages.
    pub runtime: AgentRuntimeHandle,
    /// Task that forwards the runtime result into the TUI event channel.
    pub task: JoinHandle<()>,
}

/// Result returned by a non-interactive agent execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadlessRunOutcome {
    /// Plain assistant text collected from the final conversation.
    pub assistant_text: String,
    /// Final stop reason reported by the agent runtime.
    pub stop_reason: StopReason,
}

impl std::fmt::Debug for AgentDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentDriver")
            .field("runtime_config", &self.runtime_config)
            .field("workspace", &self.workspace)
            .field("agents_dir", &self.agents_dir)
            .field("compact_pending", &self.compact_pending)
            .field("agent_catalog", &"AgentProfileCatalog")
            .field("active_agent", &self.active_agent)
            .field("mcp_tools", &self.mcp_tools.lock().unwrap().len())
            .finish()
    }
}

impl AgentDriver {
    /// Create an `AgentDriver` with agents stored under `<workspace>/.joker/agents`.
    #[must_use]
    pub fn new(runtime_config: RuntimeConfig, workspace: impl Into<PathBuf>) -> Self {
        let workspace: PathBuf = workspace.into();
        Self::new_with_agents_dir(
            runtime_config,
            workspace.clone(),
            workspace.join(".joker").join("agents"),
        )
    }

    /// Create an `AgentDriver` with a custom agents directory.
    #[must_use]
    pub fn new_with_agents_dir(
        runtime_config: RuntimeConfig,
        workspace: impl Into<PathBuf>,
        agents_dir: impl Into<PathBuf>,
    ) -> Self {
        let workspace: PathBuf = workspace.into();
        let agents_dir: PathBuf = agents_dir.into();
        let agent_catalog = AgentProfileCatalog::new(agents_dir.clone()).with_profiles(
            runtime_config
                .agent_configs
                .iter()
                .map(|(name, cfg)| (name.clone(), profile_spec_from_config(cfg))),
        );
        let _ = agent_catalog.ensure_builtin_constraint_files();

        let mut engine = PermissionEngine::new();
        for profile in agent_catalog.permissions() {
            engine.register(profile);
        }

        Self {
            runtime_config,
            credential_store: joker::CredentialStore::new(),
            workspace,
            agents_dir,
            compact_pending: false,
            agent_catalog,
            permission_engine: engine,
            active_agent: "build".into(),
            mcp_tools: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// Update the runtime configuration used to build subsequent agents.
    pub fn set_runtime_config(&mut self, runtime_config: RuntimeConfig) {
        self.runtime_config = runtime_config;
    }

    /// Set the credential store consulted when building models.
    ///
    /// Attached to the active route so model construction resolves keys
    /// through the unified auth chain (value > store > env).
    pub fn set_credential_store(&mut self, store: joker::CredentialStore) {
        self.credential_store = store;
    }

    /// Return the name of the currently active agent profile.
    #[must_use]
    pub fn active_agent(&self) -> &str {
        &self.active_agent
    }

    /// Switch the active agent profile by name.
    pub fn set_active_agent(&mut self, agent_name: String) {
        self.active_agent = agent_name;
    }

    /// Return a reference to the permission engine.
    #[must_use]
    pub fn permission_engine(&self) -> &PermissionEngine {
        &self.permission_engine
    }

    /// Return a reference to the workspace path.
    #[must_use]
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    /// Return a reference to the agents configuration directory.
    #[must_use]
    pub fn agents_dir(&self) -> &PathBuf {
        &self.agents_dir
    }

    /// Return a mutable reference to the permission engine.
    pub fn permission_engine_mut(&mut self) -> &mut PermissionEngine {
        &mut self.permission_engine
    }

    /// Spawn an async agent run for the given prompt.
    pub fn spawn_run(
        &self,
        prompt: String,
        cancellation_token: CancellationToken,
        tx: tokio::sync::mpsc::UnboundedSender<UiEvent>,
        approval_channel: SharedApprovalChannel,
    ) -> Result<SpawnedAgentRun, TuiError> {
        debug!(target: "tui.driver", "spawning agent runtime");
        let observer = Arc::new(ChannelObserver::new(tx.clone()));
        let agent = self.build_agent(observer, Some(approval_channel.clone()))?;
        let request = RunRequest::new(prompt).with_cancellation_token(cancellation_token);
        let runtime = AgentRuntime::new(agent);
        let (runtime_handle, join) = runtime.spawn(request);
        let task = tokio::spawn(async move {
            let result = match join.await {
                Ok(result) => result.map_err(|error| error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send(UiEvent::RunCompleted(result));
        });
        Ok(SpawnedAgentRun {
            runtime: runtime_handle,
            task,
        })
    }

    /// Spawn an async agent run that continues from an existing conversation.
    pub fn spawn_run_with_conversation(
        &self,
        conversation: joker::Conversation,
        cancellation_token: CancellationToken,
        tx: tokio::sync::mpsc::UnboundedSender<UiEvent>,
        approval_channel: SharedApprovalChannel,
    ) -> Result<SpawnedAgentRun, TuiError> {
        debug!(target: "tui.driver", message_count = conversation.messages().len(), "spawning agent runtime with conversation");
        let observer = Arc::new(ChannelObserver::new(tx.clone()));
        let agent = self.build_agent(observer, Some(approval_channel.clone()))?;
        let request =
            RunRequest::with_conversation(conversation).with_cancellation_token(cancellation_token);
        let runtime = AgentRuntime::new(agent);
        let (runtime_handle, join) = runtime.spawn(request);
        let task = tokio::spawn(async move {
            let result = match join.await {
                Ok(result) => result.map_err(|error| error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send(UiEvent::RunCompleted(result));
        });
        Ok(SpawnedAgentRun {
            runtime: runtime_handle,
            task,
        })
    }

    /// Set or clear the pending-compaction flag for the next agent run.
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

    /// Run one prompt without a TUI and return the assistant's final text.
    ///
    /// Non-interactive runs intentionally do not install an approval channel:
    /// tool requests that need approval are denied by policy instead of waiting
    /// for a user response that can never arrive.
    pub async fn run_headless(&self, prompt: String) -> Result<HeadlessRunOutcome, TuiError> {
        debug!(target: "tui.driver", "starting headless agent run");
        let agent = self.build_agent(Arc::new(NoopObserver), None)?;
        let outcome = agent
            .run(RunRequest::new(prompt))
            .await
            .map_err(|error| TuiError::Agent(error.to_string()))?;
        Ok(HeadlessRunOutcome {
            assistant_text: collect_assistant_text(&outcome.conversation),
            stop_reason: outcome.stop_reason,
        })
    }

    fn build_agent(
        &self,
        observer: Arc<dyn Observer>,
        approval_channel: Option<SharedApprovalChannel>,
    ) -> Result<Agent, TuiError> {
        let model = self.build_model()?;
        let mut agent = Agent::new(model).with_observer(observer.clone());
        if let Some(channel) = &approval_channel {
            agent = agent.with_approval_channel(channel.clone());
        }

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
        let safety_policy = PermissionPolicy::new().with_rules(vec![
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
        let engine_policy = if let Some(channel) = approval_channel {
            self.permission_engine
                .policy_for_with_channel(self.active_agent.clone(), channel)
        } else {
            self.permission_engine.policy_for(self.active_agent.clone())
        };
        let policy = Arc::new(ChainPolicy {
            first: Arc::new(safety_policy),
            second: engine_policy,
        });
        agent = agent.with_policy(policy);

        // Wrap context builder with system prompt from agent profile
        let system_prompt = self
            .agent_catalog
            .system_prompt(&self.active_agent, None, None);
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
                Ok(Arc::new(ScriptedModel::new(self.scripted_steps())) as Arc<dyn joker::Model>)
            }
            ProviderSelection::Route(route) => {
                let model = if route.default_model.is_empty() {
                    "model"
                } else {
                    &route.default_model
                };
                route
                    .clone()
                    .with_credential_store(self.credential_store.clone())
                    .build_model_for(model)
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

fn collect_assistant_text(conversation: &joker::Conversation) -> String {
    conversation
        .messages()
        .iter()
        .rev()
        .find(|message| matches!(&message.role, joker::Role::Assistant))
        .map(|message| {
            message
                .content
                .iter()
                .filter_map(|content| match content {
                    joker::Content::Text(text) => Some(text.text.as_str()),
                    _ => None,
                })
                .collect::<String>()
        })
        .unwrap_or_default()
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
/// Also evaluates shell command chains segment-by-segment (reference: gemini-cli's
/// `checkShellCommand` which splits on separators and checks each subcommand).
///
/// Chain detection: a command like `git log ; rm -rf /` is split on `;`, `&&`, `||`, `|`
/// and each segment is checked independently. If any segment is denied, the entire
/// command is denied. If any segment triggers Ask, the entire command is downgraded.
struct ChainPolicy {
    first: Arc<dyn ToolPolicy>,
    second: Arc<dyn ToolPolicy>,
}

/// Shell operators that trigger chain detection.
const CHAIN_SEPARATORS: &[&str] = &["&&", "||", ";", "|", "`", "$("];

/// Arity-aware trusted prefixes: "git log" matches only "git log ...", not "git push".
/// Reference: CodeWhale's BashArityDict for distinction between "git" and "git log".
const ARITY_TRUSTED_COMMANDS: &[&str] = &[
    // Rust toolchain
    "cargo test",
    "cargo build",
    "cargo check",
    "cargo fmt",
    "cargo clippy",
    "cargo doc",
    "cargo run",
    // Git (reading)
    "git status",
    "git diff",
    "git log",
    "git show",
    "git branch",
    "git stash",
    "git blame",
    // File inspection
    "ls ",
    "cat ",
    "head ",
    "tail ",
    "echo ",
    "pwd",
    "whoami",
    "date",
    "which ",
    "type ",
    "file ",
    // Directory creation
    "mkdir ",
    "touch ",
];

impl ChainPolicy {
    /// Split a shell command into segments on chain separators.
    /// Reference: gemini-cli's `splitCommands()` in shell-utils.ts.
    fn split_command_chain(command: &str) -> Vec<String> {
        let mut segments = vec![command.to_string()];
        for sep in CHAIN_SEPARATORS {
            let mut new_segments = Vec::new();
            for seg in &segments {
                let split: Vec<&str> = seg.split(sep).collect();
                new_segments.extend(split.iter().map(|s| s.to_string()));
            }
            segments = new_segments;
        }
        segments.retain(|s| !s.trim().is_empty());
        segments
    }

    /// Check if a command prefix is trusted with arity awareness.
    /// "git log" matches "git log --oneline" but NOT "git push".
    /// Reference: CodeWhale's BashArityDict.
    fn is_arity_trusted(command: &str) -> bool {
        let trimmed = command.trim();
        ARITY_TRUSTED_COMMANDS
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
    }

    /// Check if a command contains redirection/pipeline operators.
    fn has_redirect_operators(command: &str) -> bool {
        const REDIRECT_PATTERNS: &[&str] = &["|", ">", "$(", "`"];
        REDIRECT_PATTERNS.iter().any(|op| command.contains(op))
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
                    // ── Shell command chain detection ──────────────────
                    // If this is a shell command, check each segment.
                    // Reference: gemini-cli's checkShellCommand() which:
                    // 1. Splits on separators
                    // 2. Checks each segment independently
                    // 3. DENY from any segment is terminal
                    // 4. ASK_USER from any segment downgrades ALLOW
                    if request2.invocation.name.as_str() == "shell"
                        && let Some(cmd) = request2
                            .invocation
                            .arguments
                            .get("command")
                            .and_then(|v| v.as_str())
                    {
                        let segments = Self::split_command_chain(cmd);

                        // Check each segment's trust level
                        let mut worst_decision: Option<ToolDecision> = None;
                        for segment in &segments {
                            let seg_trimmed = segment.trim();
                            if seg_trimmed.is_empty() {
                                continue;
                            }

                            // Check if this individual segment is arity-trusted
                            let is_trusted = Self::is_arity_trusted(seg_trimmed);

                            // If any segment is not trusted, downgrade the whole command
                            if !is_trusted {
                                // Not arity-trusted: check redirect/pipeline
                                if Self::has_redirect_operators(seg_trimmed) {
                                    worst_decision = Some(ToolDecision::Ask {
                                        request_id: "chain-redirect".into(),
                                        reason: format!(
                                            "shell segment '{:.60}' contains redirect/pipeline operators",
                                            seg_trimmed
                                        ),
                                    });
                                } else if !matches!(worst_decision, Some(ToolDecision::Deny { .. }))
                                {
                                    worst_decision = Some(ToolDecision::Ask {
                                        request_id: "chain-unknown".into(),
                                        reason: format!(
                                            "shell segment '{:.60}' is not in trusted command list",
                                            seg_trimmed
                                        ),
                                    });
                                }
                            }
                        }

                        if let Some(decision) = worst_decision {
                            return Ok(decision);
                        }
                    }

                    self.second.evaluate(request2).await
                }
            }
        })
    }
}

/// Convert an `AgentProfileConfig` from the config layer into a core profile spec.
fn profile_spec_from_config(cfg: &joker_config::AgentProfileConfig) -> AgentProfileSpec {
    AgentProfileSpec {
        model: cfg.model.clone(),
        system: cfg.system.clone(),
        tools: cfg
            .tools
            .iter()
            .map(|(name, tool_cfg)| {
                (
                    name.clone(),
                    AgentToolPermissionSpec {
                        enabled: tool_cfg.enabled,
                        permission: tool_cfg.permission.clone(),
                    },
                )
            })
            .collect(),
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
