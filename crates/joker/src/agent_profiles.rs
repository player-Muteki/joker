//! Built-in agent profiles and their permission configurations.
//!
//! Three agents are defined per OUTLINE.md Section 8:
//!
//! | Agent | Read/Grep/Glob | Write/Edit | Shell | ApplyPatch | WebSearch/Fetch | Memory |
//! |-------|---------------|------------|-------|------------|-----------------|--------|
//! | plan  | AutoAccept    | Disabled   | Disabled | Disabled  | Ask             | AutoAccept |
//! | build | Ask           | Ask        | Ask   | Ask        | Ask             | Ask |
//! | yolo  | AutoAccept    | AutoAccept | AutoAccept | AutoAccept | Ask         | AutoAccept |
//!
//! The plan agent uses `hard_permission: Disabled` on all mutating tools,
//! making the restriction non-overridable by user config or session grants.

use std::{
    collections::{BTreeMap, HashMap},
    fs, io,
    path::PathBuf,
};

use crate::permission_engine::{AgentPermission, HardPermissionRule, PermissionSetting};
use crate::tool::ToolName;

/// Catalog of built-in and configured agent profiles.
#[derive(Clone, Debug)]
pub struct AgentProfileCatalog {
    agents_dir: PathBuf,
    profiles: BTreeMap<String, AgentProfileSpec>,
}

/// Config-neutral agent profile specification.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentProfileSpec {
    /// Override model for this agent.
    pub model: Option<String>,
    /// System prompt for this agent.
    pub system: Option<String>,
    /// Per-tool permission overrides keyed by tool name.
    pub tools: BTreeMap<String, AgentToolPermissionSpec>,
}

/// Config-neutral permission specification for one tool in an agent profile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentToolPermissionSpec {
    /// Whether the tool is enabled for this agent.
    pub enabled: Option<bool>,
    /// Permission level (`"ask"`, `"auto-accept"`, `"disabled"`).
    pub permission: Option<String>,
}

impl AgentProfileCatalog {
    /// Create a catalog rooted at an agents directory.
    #[must_use]
    pub fn new(agents_dir: impl Into<PathBuf>) -> Self {
        Self {
            agents_dir: agents_dir.into(),
            profiles: BTreeMap::new(),
        }
    }

    /// Add or replace a configured profile.
    #[must_use]
    pub fn with_profile(mut self, name: impl Into<String>, spec: AgentProfileSpec) -> Self {
        self.profiles.insert(name.into(), spec);
        self
    }

    /// Add configured profiles.
    #[must_use]
    pub fn with_profiles<I, N>(mut self, profiles: I) -> Self
    where
        I: IntoIterator<Item = (N, AgentProfileSpec)>,
        N: Into<String>,
    {
        for (name, spec) in profiles {
            self.profiles.insert(name.into(), spec);
        }
        self
    }

    /// Write missing built-in constraint files without overwriting user edits.
    pub fn ensure_builtin_constraint_files(&self) -> io::Result<()> {
        fs::create_dir_all(&self.agents_dir)?;
        for name in ["plan", "build", "yolo"] {
            let path = self.agents_dir.join(format!("{name}_agent.md"));
            if !path.exists() {
                let content = builtin_constraint_file_content(name);
                if !content.is_empty() {
                    fs::write(path, content)?;
                }
            }
        }
        Ok(())
    }

    /// Materialize all built-in and configured profile permissions.
    #[must_use]
    pub fn permissions(&self) -> Vec<AgentPermission> {
        let mut permissions = builtin_agent_profiles(&self.agents_dir);
        permissions.extend(
            self.profiles
                .iter()
                .filter(|(name, _)| !is_builtin_agent(name))
                .map(|(name, spec)| self.permission_from_spec(name, spec)),
        );
        permissions
    }

    /// Build the system prompt for an agent from project context, profile constraints, and memory.
    #[must_use]
    pub fn system_prompt(
        &self,
        agent_name: &str,
        project_context: Option<&str>,
        memory: Option<&str>,
    ) -> String {
        let mut parts: Vec<String> = Vec::new();

        if let Some(ctx) = project_context
            && !ctx.trim().is_empty()
        {
            parts.push(format!("## Project Context\n\n{ctx}"));
        }

        if let Some(constraint) = self.constraint_content(agent_name)
            && !constraint.trim().is_empty()
        {
            parts.push(constraint.to_string());
        }

        if let Some(mem) = memory
            && !mem.trim().is_empty()
        {
            parts.push(format!("## Memory\n\n{mem}"));
        }

        parts.join("\n\n")
    }

    /// Return configured system text or built-in constraint content for an agent.
    #[must_use]
    pub fn constraint_content(&self, agent_name: &str) -> Option<&str> {
        self.profiles
            .get(agent_name)
            .and_then(|spec| spec.system.as_deref())
            .or_else(|| {
                let content = builtin_constraint_file_content(agent_name);
                (!content.is_empty()).then_some(content)
            })
    }

    fn permission_from_spec(&self, name: &str, spec: &AgentProfileSpec) -> AgentPermission {
        let tool_permissions = spec
            .tools
            .iter()
            .map(|(tool_name, tool_spec)| {
                (
                    ToolName::new(tool_name),
                    permission_setting_from_spec(tool_spec),
                )
            })
            .collect();

        AgentPermission {
            agent_name: name.to_string(),
            tool_permissions,
            constraint_file: self.agents_dir.join(format!("{name}_agent.md")),
            hard_permission: None,
            hard_permission_rules: Vec::new(),
            model: spec.model.clone(),
        }
    }
}

fn is_builtin_agent(name: &str) -> bool {
    matches!(name, "plan" | "build" | "yolo")
}

fn permission_setting_from_spec(spec: &AgentToolPermissionSpec) -> PermissionSetting {
    match spec.permission.as_deref() {
        Some("auto-accept" | "auto_accept" | "auto") => PermissionSetting::AutoAccept,
        Some("ask") => PermissionSetting::Ask,
        Some("disabled" | "disable" | "deny" | "none") => PermissionSetting::Disabled,
        _ if spec.enabled == Some(false) => PermissionSetting::Disabled,
        _ => PermissionSetting::Ask,
    }
}

/// Create the three built-in agent profiles.
#[must_use]
pub fn builtin_agent_profiles(agents_dir: &std::path::Path) -> Vec<AgentPermission> {
    vec![
        plan_profile(agents_dir),
        build_profile(agents_dir),
        yolo_profile(agents_dir),
    ]
}

fn plan_profile(agents_dir: &std::path::Path) -> AgentPermission {
    let mut perms = HashMap::new();
    // Read-only tools: auto-accept
    perms.insert(tn("list_files"), PermissionSetting::AutoAccept);
    perms.insert(tn("read_file"), PermissionSetting::AutoAccept);
    perms.insert(tn("grep"), PermissionSetting::AutoAccept);
    perms.insert(tn("glob"), PermissionSetting::AutoAccept);
    // Memory: auto-accept (plan can read/write memory)
    perms.insert(tn("memory_read"), PermissionSetting::AutoAccept);
    perms.insert(tn("memory_write"), PermissionSetting::AutoAccept);
    // Mutating tools: disabled (enforced by hard_permission + rules too)
    perms.insert(tn("write_file"), PermissionSetting::Disabled);
    perms.insert(tn("edit_file"), PermissionSetting::Disabled);
    perms.insert(tn("apply_patch"), PermissionSetting::Disabled);
    perms.insert(tn("shell"), PermissionSetting::Disabled);
    perms.insert(tn("todo_write"), PermissionSetting::Disabled);
    // Network: ask
    perms.insert(tn("web_search"), PermissionSetting::Ask);
    perms.insert(tn("fetch_url"), PermissionSetting::Ask);

    AgentPermission {
        agent_name: "plan".into(),
        tool_permissions: perms,
        constraint_file: agents_dir.join("plan_agent.md"),
        hard_permission: Some(PermissionSetting::Disabled), // blocks all mutating tools
        // Path-level hard rules (MiMo-Code style): plan files are the only exception
        hard_permission_rules: vec![HardPermissionRule {
            tool_pattern: "*".into(),
            resource_pattern: "plans/*.md".into(),
            setting: PermissionSetting::AutoAccept,
        }],
        model: None,
    }
}

fn build_profile(agents_dir: &std::path::Path) -> AgentPermission {
    let mut perms = HashMap::new();
    // All tools: ask (user confirms every action)
    for name in all_tool_names() {
        perms.insert(name, PermissionSetting::Ask);
    }

    AgentPermission {
        agent_name: "build".into(),
        tool_permissions: perms,
        constraint_file: agents_dir.join("build_agent.md"),
        hard_permission: None,
        hard_permission_rules: Vec::new(),
        model: None,
    }
}

fn yolo_profile(agents_dir: &std::path::Path) -> AgentPermission {
    let mut perms = HashMap::new();
    // Read: auto-accept
    perms.insert(tn("list_files"), PermissionSetting::AutoAccept);
    perms.insert(tn("read_file"), PermissionSetting::AutoAccept);
    perms.insert(tn("grep"), PermissionSetting::AutoAccept);
    perms.insert(tn("glob"), PermissionSetting::AutoAccept);
    // Write: auto-accept
    perms.insert(tn("write_file"), PermissionSetting::AutoAccept);
    perms.insert(tn("edit_file"), PermissionSetting::AutoAccept);
    perms.insert(tn("apply_patch"), PermissionSetting::AutoAccept);
    // Shell: auto-accept
    perms.insert(tn("shell"), PermissionSetting::AutoAccept);
    // Todo: auto-accept
    perms.insert(tn("todo_write"), PermissionSetting::AutoAccept);
    // Memory: auto-accept
    perms.insert(tn("memory_read"), PermissionSetting::AutoAccept);
    perms.insert(tn("memory_write"), PermissionSetting::AutoAccept);
    // Network: ask (external services)
    perms.insert(tn("web_search"), PermissionSetting::Ask);
    perms.insert(tn("fetch_url"), PermissionSetting::Ask);

    AgentPermission {
        agent_name: "yolo".into(),
        tool_permissions: perms,
        constraint_file: agents_dir.join("yolo_agent.md"),
        hard_permission: None,
        hard_permission_rules: Vec::new(),
        model: None,
    }
}

fn tn(name: &str) -> ToolName {
    ToolName::new(name)
}

fn all_tool_names() -> Vec<ToolName> {
    vec![
        tn("list_files"),
        tn("read_file"),
        tn("grep"),
        tn("glob"),
        tn("write_file"),
        tn("edit_file"),
        tn("apply_patch"),
        tn("shell"),
        tn("todo_write"),
        tn("web_search"),
        tn("fetch_url"),
        tn("memory_read"),
        tn("memory_write"),
    ]
}

/// Return the constraint file content for built-in agents.
/// These are written to `~/.joker/agents/` on first run if the files
/// don't already exist.
pub fn builtin_constraint_file_content(agent_name: &str) -> &'static str {
    match agent_name {
        "plan" => PLAN_AGENT_MD,
        "build" => BUILD_AGENT_MD,
        "yolo" => YOLO_AGENT_MD,
        _ => "",
    }
}

const PLAN_AGENT_MD: &str = r#"# Plan Agent

You are a planning agent. Your role is to analyze code, explore the codebase,
and produce structured implementation plans.

## Capabilities
- Read files, search with grep, list directories.
- Search the web for documentation and references.
- Read and write memory for context persistence.

## Limitations
You CANNOT edit files, execute shell commands, or write code. If you need to
make changes, produce a clear plan and ask the user to switch to the build agent.

## Output Format
Always output a structured plan with numbered steps before requesting agent switch.
Each step should include: what to change, which files are affected, and why.
"#;

const BUILD_AGENT_MD: &str = r#"# Build Agent

You are a build agent. Your role is to implement changes based on plans or user
instructions.

## Capabilities
- Read, write, edit, and patch files.
- Execute shell commands (build, test, git).
- Search code with grep.
- Fetch web resources.

## Behavior
- Explain what you are about to do before doing it.
- Ask for confirmation before running destructive commands.
- Commit changes with descriptive messages after each logical unit of work.
- Report what you changed and why after completing tasks.
"#;

const YOLO_AGENT_MD: &str = r#"# YOLO Agent

You are a YOLO (autonomous) agent. You execute tasks without asking for
confirmation.

## Capabilities
- All file operations, shell commands, and code search are auto-approved.
- Web search and fetch require confirmation (external network access).

## Behavior
- Always state what you did after completing each task.
- Report any errors or unexpected results immediately.
- Proceed efficiently without unnecessary pauses.
- If you encounter something unexpected, explain before continuing.
"#;
