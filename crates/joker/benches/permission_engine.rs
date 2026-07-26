use std::collections::HashMap;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use joker::{
    AgentPermission, PermissionEngine, PermissionSetting, Tool, ToolAnnotations, ToolDefinition,
    ToolFuture, ToolInvocation, ToolName, ToolOutput, ToolRegistry,
};
use serde_json::json;

struct DummyTool(ToolDefinition);

impl Tool for DummyTool {
    fn definition(&self) -> ToolDefinition {
        self.0.clone()
    }
    fn call(&self, _invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async { Ok(ToolOutput::new(json!({}))) })
    }
}

fn plan_profile() -> AgentPermission {
    let mut perms = HashMap::new();
    perms.insert(ToolName::new("read_file"), PermissionSetting::AutoAccept);
    perms.insert(ToolName::new("write_file"), PermissionSetting::Disabled);
    AgentPermission {
        agent_name: "plan".into(),
        tool_permissions: perms,
        constraint_file: std::path::PathBuf::from("plan_agent.md"),
        hard_permission: Some(PermissionSetting::Disabled),
        hard_permission_rules: Vec::new(),
        model: None,
    }
}

fn yolo_profile() -> AgentPermission {
    let mut perms = HashMap::new();
    perms.insert(ToolName::new("write_file"), PermissionSetting::AutoAccept);
    perms.insert(ToolName::new("web_search"), PermissionSetting::Ask);
    AgentPermission {
        agent_name: "yolo".into(),
        tool_permissions: perms,
        constraint_file: std::path::PathBuf::from("yolo_agent.md"),
        hard_permission: None,
        hard_permission_rules: Vec::new(),
        model: None,
    }
}

fn bench_evaluate_allow(c: &mut Criterion) {
    let mut engine = PermissionEngine::new();
    engine.register(yolo_profile());

    c.benchmark_group("permission_engine")
        .bench_function("evaluate_allow", |b| {
            b.iter(|| {
                let _ = engine.evaluate("yolo", &ToolName::new("write_file"), true, None);
            })
        });
}

fn bench_evaluate_deny(c: &mut Criterion) {
    let mut engine = PermissionEngine::new();
    engine.register(plan_profile());

    c.benchmark_group("permission_engine")
        .bench_function("evaluate_deny_hard_permission", |b| {
            b.iter(|| {
                let _ = engine.evaluate("plan", &ToolName::new("write_file"), true, None);
            })
        });
}

fn bench_evaluate_ask(c: &mut Criterion) {
    let mut engine = PermissionEngine::new();
    engine.register(yolo_profile());

    c.benchmark_group("permission_engine")
        .bench_function("evaluate_ask", |b| {
            b.iter(|| {
                let _ = engine.evaluate("yolo", &ToolName::new("web_search"), true, None);
            })
        });
}

fn bench_evaluate_unknown_agent(c: &mut Criterion) {
    let engine = PermissionEngine::new();

    c.benchmark_group("permission_engine")
        .bench_function("evaluate_unknown_agent_fallback", |b| {
            b.iter(|| {
                let _ = engine.evaluate("unknown", &ToolName::new("write_file"), true, None);
            })
        });
}

fn bench_materialize_tools(c: &mut Criterion) {
    let mut engine = PermissionEngine::new();
    engine.register(plan_profile());

    let registry = {
        let mut r = ToolRegistry::new();
        for i in 0..5 {
            let def = ToolDefinition {
                name: ToolName::new(format!("tool_{i}")),
                description: "bench".into(),
                input_schema: json!({"type":"object"}),
                annotations: ToolAnnotations::default(),
            };
            let _ = r.insert_arc(Arc::new(DummyTool(def)));
        }
        r
    };

    c.benchmark_group("permission_engine")
        .bench_function("materialize_5_tools", |b| {
            b.iter(|| {
                let _ = engine.materialize_tools("plan", &registry);
            })
        });
}

fn bench_session_grant(c: &mut Criterion) {
    let mut engine = PermissionEngine::new();
    engine.register(yolo_profile());

    c.benchmark_group("permission_engine")
        .bench_function("grant_and_check_session", |b| {
            let tool = ToolName::new("web_search");
            b.iter(|| {
                engine.grant_session("yolo", tool.clone());
                let _ = engine.evaluate("yolo", &tool, true, None);
            })
        });
}

criterion_group!(
    name = permission_engine;
    config = Criterion::default().sample_size(100);
    targets = bench_evaluate_allow, bench_evaluate_deny, bench_evaluate_ask,
             bench_evaluate_unknown_agent, bench_materialize_tools, bench_session_grant
);
criterion_main!(permission_engine);
