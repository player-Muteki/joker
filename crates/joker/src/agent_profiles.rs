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

use std::collections::HashMap;

use crate::permission_engine::{AgentPermission, HardPermissionRule, PermissionSetting};
use crate::tool::ToolName;

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
