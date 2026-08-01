use std::collections::BTreeMap;

use joker::{
    AgentProfileCatalog, AgentProfileSpec, AgentToolPermissionSpec, PermissionDecision,
    PermissionEngine, PermissionSetting, ToolAnnotations, ToolDefinition, ToolExecution, ToolFn,
    ToolFuture, ToolInvocation, ToolName, ToolOutput, ToolRegistry, builtin_agent_profiles,
    builtin_constraint_file_content,
};
use serde_json::json;

// ── Helpers ────────────────────────────────────────────────────────────────

fn make_tool(
    name: &'static str,
    mutating: bool,
) -> ToolFn<fn(ToolInvocation) -> ToolFuture<'static>> {
    ToolFn::new(
        ToolDefinition {
            name: ToolName::new(name),
            description: name.into(),
            input_schema: json!({"type": "object"}),
            annotations: ToolAnnotations {
                execution: ToolExecution::Sequential,
                mutating,
                ..ToolAnnotations::default()
            },
        },
        |invocation: ToolInvocation| -> ToolFuture<'static> {
            Box::pin(async move { Ok(ToolOutput::new(json!({"name": invocation.name.as_str()}))) })
        },
    )
}

fn all_tool_names() -> Vec<(&'static str, bool)> {
    vec![
        ("list_files", false),
        ("read_file", false),
        ("grep", false),
        ("glob", false),
        ("write_file", true),
        ("edit_file", true),
        ("apply_patch", true),
        ("shell", true),
        ("todo_write", true),
        ("web_search", false),
        ("fetch_url", false),
        ("memory_read", false),
        ("memory_write", false),
    ]
}

fn make_all_tools() -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    for (name, mutating) in all_tool_names() {
        reg.insert(make_tool(name, mutating)).unwrap();
    }
    reg
}

fn find_profile<'a>(
    profiles: &'a [joker::AgentPermission],
    name: &str,
) -> &'a joker::AgentPermission {
    profiles.iter().find(|p| p.agent_name == name).unwrap()
}

fn check_perm(profile: &joker::AgentPermission, tool: &str, expected: PermissionSetting) {
    let actual = profile.tool_permissions.get(&ToolName::new(tool));
    assert_eq!(
        actual,
        Some(&expected),
        "plan agent: tool '{tool}' permission mismatch"
    );
}

fn register_profiles(engine: &mut PermissionEngine) {
    let agents_dir = std::path::Path::new("/tmp/.joker/agents");
    for profile in builtin_agent_profiles(agents_dir) {
        engine.register(profile);
    }
}

// ── Profile structure ──────────────────────────────────────────────────────

#[test]
fn test_builtin_agent_profiles_returns_three_profiles() {
    let agents_dir = std::path::Path::new("/tmp/.joker/agents");
    let profiles = builtin_agent_profiles(agents_dir);

    assert_eq!(profiles.len(), 3, "expected exactly 3 built-in profiles");

    let names: Vec<&str> = profiles.iter().map(|p| p.agent_name.as_str()).collect();
    assert_eq!(names, vec!["plan", "build", "yolo"]);
}

#[test]
fn test_plan_profile_permissions() {
    let agents_dir = std::path::Path::new("/tmp/.joker/agents");
    let profiles = builtin_agent_profiles(agents_dir);
    let plan = find_profile(&profiles, "plan");

    assert_eq!(plan.hard_permission, Some(PermissionSetting::Disabled));
    assert_eq!(plan.hard_permission_rules.len(), 1);
    assert!(
        plan.constraint_file.ends_with("plan_agent.md"),
        "constraint_file should end with plan_agent.md, got {:?}",
        plan.constraint_file
    );

    check_perm(plan, "list_files", PermissionSetting::AutoAccept);
    check_perm(plan, "read_file", PermissionSetting::AutoAccept);
    check_perm(plan, "grep", PermissionSetting::AutoAccept);
    check_perm(plan, "glob", PermissionSetting::AutoAccept);
    check_perm(plan, "memory_read", PermissionSetting::AutoAccept);
    check_perm(plan, "memory_write", PermissionSetting::AutoAccept);

    check_perm(plan, "write_file", PermissionSetting::Disabled);
    check_perm(plan, "edit_file", PermissionSetting::Disabled);
    check_perm(plan, "apply_patch", PermissionSetting::Disabled);
    check_perm(plan, "shell", PermissionSetting::Disabled);
    check_perm(plan, "todo_write", PermissionSetting::Disabled);

    check_perm(plan, "web_search", PermissionSetting::Ask);
    check_perm(plan, "fetch_url", PermissionSetting::Ask);
}

#[test]
fn test_build_profile_permissions() {
    let agents_dir = std::path::Path::new("/tmp/.joker/agents");
    let profiles = builtin_agent_profiles(agents_dir);
    let build = find_profile(&profiles, "build");

    assert_eq!(build.hard_permission, None);
    assert!(build.hard_permission_rules.is_empty());
    assert!(
        build.constraint_file.ends_with("build_agent.md"),
        "constraint_file should end with build_agent.md, got {:?}",
        build.constraint_file
    );

    for (name, _) in all_tool_names() {
        check_perm(build, name, PermissionSetting::Ask);
    }
}

#[test]
fn test_yolo_profile_permissions() {
    let agents_dir = std::path::Path::new("/tmp/.joker/agents");
    let profiles = builtin_agent_profiles(agents_dir);
    let yolo = find_profile(&profiles, "yolo");

    assert_eq!(yolo.hard_permission, None);
    assert!(yolo.hard_permission_rules.is_empty());
    assert!(
        yolo.constraint_file.ends_with("yolo_agent.md"),
        "constraint_file should end with yolo_agent.md, got {:?}",
        yolo.constraint_file
    );

    let auto_accept_tools = [
        "list_files",
        "read_file",
        "grep",
        "glob",
        "write_file",
        "edit_file",
        "apply_patch",
        "shell",
        "todo_write",
        "memory_read",
        "memory_write",
    ];
    for tool in &auto_accept_tools {
        check_perm(yolo, tool, PermissionSetting::AutoAccept);
    }

    check_perm(yolo, "web_search", PermissionSetting::Ask);
    check_perm(yolo, "fetch_url", PermissionSetting::Ask);
}

// ── Permission evaluation ──────────────────────────────────────────────────

#[test]
fn test_plan_hard_permission_blocks_mutating_tools_even_with_session_grant() {
    let mut engine = PermissionEngine::new();
    register_profiles(&mut engine);

    let write_file = ToolName::new("write_file");

    // Hard permission blocks
    let decision = engine.evaluate("plan", &write_file, true, None);
    assert!(
        matches!(decision, PermissionDecision::Deny { .. }),
        "plan hard-permission should deny write_file: got {decision:?}"
    );

    // Session grant cannot override hard permission
    engine.grant_session("plan", write_file.clone());
    let decision = engine.evaluate("plan", &write_file, true, None);
    assert!(
        matches!(decision, PermissionDecision::Deny { .. }),
        "hard-permission must win over session grant: got {decision:?}"
    );
}

#[test]
fn test_plan_auto_accepts_read_only_tools() {
    let mut engine = PermissionEngine::new();
    register_profiles(&mut engine);

    let read_file = ToolName::new("read_file");
    let decision = engine.evaluate("plan", &read_file, false, None);
    assert_eq!(decision, PermissionDecision::Allow);
}

#[test]
fn test_plan_asks_for_network_tools() {
    let mut engine = PermissionEngine::new();
    register_profiles(&mut engine);

    let web_search = ToolName::new("web_search");
    let decision = engine.evaluate("plan", &web_search, false, None);
    assert!(
        matches!(decision, PermissionDecision::Ask { .. }),
        "plan should ask for web_search: got {decision:?}"
    );
}

#[test]
fn test_build_asks_for_all_tools() {
    let mut engine = PermissionEngine::new();
    register_profiles(&mut engine);

    for (name, mutating) in all_tool_names() {
        let tool = ToolName::new(name);
        let decision = engine.evaluate("build", &tool, mutating, None);
        assert!(
            matches!(decision, PermissionDecision::Ask { .. }),
            "build should ask for '{name}': got {decision:?}"
        );
    }
}

#[test]
fn test_yolo_auto_accepts_all_tools_except_network() {
    let mut engine = PermissionEngine::new();
    register_profiles(&mut engine);

    for (name, mutating) in all_tool_names() {
        let tool = ToolName::new(name);
        let decision = engine.evaluate("yolo", &tool, mutating, None);
        if name == "web_search" || name == "fetch_url" {
            assert!(
                matches!(decision, PermissionDecision::Ask { .. }),
                "yolo should ask for '{name}': got {decision:?}"
            );
        } else {
            assert_eq!(
                decision,
                PermissionDecision::Allow,
                "yolo should auto-accept '{name}': got {decision:?}"
            );
        }
    }
}

#[test]
fn test_session_grant_overrides_ask_for_build() {
    let mut engine = PermissionEngine::new();
    register_profiles(&mut engine);

    let shell = ToolName::new("shell");

    // Initially Ask
    let decision = engine.evaluate("build", &shell, true, None);
    assert!(matches!(decision, PermissionDecision::Ask { .. }));

    // Session grant overrides Ask (not blocked by hard_permission for build)
    engine.grant_session("build", shell.clone());
    let decision = engine.evaluate("build", &shell, true, None);
    assert_eq!(decision, PermissionDecision::Allow);
}

#[test]
fn test_unknown_agent_falls_back() {
    let engine = PermissionEngine::new();

    // Mutating tool → Ask (annotation default for mutating)
    let decision = engine.evaluate("unknown", &ToolName::new("write_file"), true, None);
    assert!(matches!(decision, PermissionDecision::Ask { .. }));

    // Read-only tool → Allow (annotation default for non-mutating)
    let decision = engine.evaluate("unknown", &ToolName::new("read_file"), false, None);
    assert_eq!(decision, PermissionDecision::Allow);
}

// ── Tool registry materialization ──────────────────────────────────────────

#[test]
fn test_plan_materialize_filters_disabled_tools() {
    let mut engine = PermissionEngine::new();
    register_profiles(&mut engine);

    let all_tools = make_all_tools();
    let materialized = engine.materialize_tools("plan", &all_tools);

    let present: Vec<String> = materialized
        .definitions()
        .into_iter()
        .map(|d| d.name.as_str().to_string())
        .collect();

    // Disabled tools must be filtered out
    for tool in &[
        "write_file",
        "edit_file",
        "apply_patch",
        "shell",
        "todo_write",
    ] {
        assert!(
            !present.contains(&tool.to_string()),
            "disabled tool '{tool}' should be filtered out by plan materialize"
        );
    }

    // Read-only, memory, and network tools must be present
    for tool in &[
        "list_files",
        "read_file",
        "grep",
        "glob",
        "web_search",
        "fetch_url",
        "memory_read",
        "memory_write",
    ] {
        assert!(
            present.contains(&tool.to_string()),
            "non-disabled tool '{tool}' should be present after plan materialize"
        );
    }
}

#[test]
fn test_build_materialize_includes_all_tools() {
    let mut engine = PermissionEngine::new();
    register_profiles(&mut engine);

    let all_tools = make_all_tools();
    let materialized = engine.materialize_tools("build", &all_tools);

    let present: Vec<String> = materialized
        .definitions()
        .into_iter()
        .map(|d| d.name.as_str().to_string())
        .collect();

    // Build has no Disabled tools — all should be present
    for (name, _) in all_tool_names() {
        assert!(
            present.contains(&name.to_string()),
            "tool '{name}' should be present after build materialize"
        );
    }
}

#[test]
fn test_yolo_materialize_includes_all_tools() {
    let mut engine = PermissionEngine::new();
    register_profiles(&mut engine);

    let all_tools = make_all_tools();
    let materialized = engine.materialize_tools("yolo", &all_tools);

    let present: Vec<String> = materialized
        .definitions()
        .into_iter()
        .map(|d| d.name.as_str().to_string())
        .collect();

    // Yolo has no Disabled tools — all should be present
    for (name, _) in all_tool_names() {
        assert!(
            present.contains(&name.to_string()),
            "tool '{name}' should be present after yolo materialize"
        );
    }
}

// ── Constraint file content ────────────────────────────────────────────────

#[test]
fn test_builtin_constraint_file_content_plan() {
    let content = builtin_constraint_file_content("plan");
    assert!(content.contains("You are a planning agent"));
    assert!(content.contains("CANNOT edit files"));
    assert!(content.contains("produce a clear plan"));
    assert!(!content.is_empty());
}

#[test]
fn test_builtin_constraint_file_content_build() {
    let content = builtin_constraint_file_content("build");
    assert!(content.contains("You are a build agent"));
    assert!(content.contains("Ask for confirmation"));
    assert!(!content.is_empty());
}

#[test]
fn test_builtin_constraint_file_content_yolo() {
    let content = builtin_constraint_file_content("yolo");
    assert!(content.contains("YOLO (autonomous) agent"));
    assert!(content.contains("auto-approved"));
    assert!(content.contains("Web search and fetch require confirmation"));
    assert!(!content.is_empty());
}

#[test]
fn test_builtin_constraint_file_content_unknown() {
    assert_eq!(builtin_constraint_file_content("unknown"), "");
}

// ── Agent profile catalog ───────────────────────────────────────────────────

#[test]
fn test_agent_profile_catalog_materializes_builtin_and_configured_profiles() {
    let agents_dir = std::path::Path::new("/tmp/.joker/agents");
    let catalog = AgentProfileCatalog::new(agents_dir).with_profile(
        "review",
        AgentProfileSpec {
            model: Some("review-model".into()),
            system: Some("You review code.".into()),
            tools: BTreeMap::from([
                (
                    "read_file".into(),
                    AgentToolPermissionSpec {
                        enabled: None,
                        permission: Some("auto-accept".into()),
                    },
                ),
                (
                    "write_file".into(),
                    AgentToolPermissionSpec {
                        enabled: Some(false),
                        permission: None,
                    },
                ),
            ]),
        },
    );

    let profiles = catalog.permissions();
    let names: Vec<&str> = profiles.iter().map(|p| p.agent_name.as_str()).collect();
    assert_eq!(names, vec!["plan", "build", "yolo", "review"]);

    let review = find_profile(&profiles, "review");
    assert_eq!(review.model.as_deref(), Some("review-model"));
    check_perm(review, "read_file", PermissionSetting::AutoAccept);
    check_perm(review, "write_file", PermissionSetting::Disabled);
    assert!(review.constraint_file.ends_with("review_agent.md"));
}

#[test]
fn test_agent_profile_catalog_writes_missing_builtin_constraints_without_overwrite() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let agents_dir = temp_dir.path().join("agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    let plan_path = agents_dir.join("plan_agent.md");
    std::fs::write(&plan_path, "custom plan").unwrap();

    let catalog = AgentProfileCatalog::new(&agents_dir);
    catalog.ensure_builtin_constraint_files().unwrap();

    assert_eq!(std::fs::read_to_string(plan_path).unwrap(), "custom plan");
    assert!(agents_dir.join("build_agent.md").exists());
    assert!(agents_dir.join("yolo_agent.md").exists());
}

#[test]
fn test_agent_profile_catalog_uses_configured_system_prompt() {
    let catalog = AgentProfileCatalog::new("/tmp/.joker/agents").with_profile(
        "review",
        AgentProfileSpec {
            system: Some("Review only changed code.".into()),
            ..AgentProfileSpec::default()
        },
    );

    let prompt = catalog.system_prompt("review", Some("Project facts"), Some("Memory facts"));
    assert!(prompt.contains("## Project Context\n\nProject facts"));
    assert!(prompt.contains("Review only changed code."));
    assert!(prompt.contains("## Memory\n\nMemory facts"));
}

#[test]
fn test_plan_hard_permission_path_rule_allows_plans_md() {
    let mut engine = PermissionEngine::new();
    register_profiles(&mut engine);

    let edit_file = ToolName::new("edit_file");
    let args = serde_json::json!({"file_path": "plans/my_plan.md"});

    // edit_file is Disabled at the tool level, but the hard_permission_rules
    // have a path-precise rule: `*` on `plans/*.md` → AutoAccept.
    // The evaluate function checks hard_permission_rules first (level 1),
    // then simple hard_permission (level 1b), so the path rule wins.
    let decision = engine.evaluate("plan", &edit_file, true, Some(&args));
    assert_eq!(
        decision,
        PermissionDecision::Allow,
        "plan should allow edit_file on plans/*.md via hard-permission rule: got {decision:?}"
    );

    // Same tool, different path → blocked by blanket hard_permission
    let args = serde_json::json!({"file_path": "src/main.rs"});
    let decision = engine.evaluate("plan", &edit_file, true, Some(&args));
    assert!(
        matches!(decision, PermissionDecision::Deny { .. }),
        "plan should deny edit_file on non-plans path: got {decision:?}"
    );
}
