use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use joker::{
    ToolAnnotations, ToolDefinition, ToolExecution, ToolFn, ToolFuture, ToolInvocation, ToolName,
    ToolOutput, ToolRegistry,
};
use serde_json::json;

fn make_tool_fn(name: String) -> ToolFn<fn(ToolInvocation) -> ToolFuture<'static>> {
    ToolFn::new(
        ToolDefinition {
            name: ToolName::new(name),
            description: "bench".into(),
            input_schema: json!({"type": "object"}),
            annotations: ToolAnnotations {
                execution: ToolExecution::Sequential,
                mutating: false,
                timeout: None,
                ..ToolAnnotations::default()
            },
        },
        |invocation: ToolInvocation| -> ToolFuture<'static> {
            let output = invocation.arguments;
            Box::pin(async move { Ok(ToolOutput::new(output)) })
        },
    )
}

fn bench_registry_insert(c: &mut Criterion) {
    c.benchmark_group("tool_dispatch")
        .bench_function("insert_10_tools", |b| {
            b.iter_batched(
                ToolRegistry::new,
                |mut registry| {
                    for i in 0..10 {
                        let _ = registry.insert(make_tool_fn(format!("tool_{i}")));
                    }
                    registry
                },
                BatchSize::SmallInput,
            )
        });
}

fn bench_registry_lookup(c: &mut Criterion) {
    let registry = {
        let mut r = ToolRegistry::new();
        for i in 0..20 {
            let _ = r.insert(make_tool_fn(format!("tool_{i}")));
        }
        r
    };

    c.benchmark_group("tool_dispatch")
        .bench_function("lookup_by_name_20_tools", |b| {
            b.iter(|| {
                let _ = registry.get(&ToolName::new("tool_15"));
            })
        });
}

fn bench_registry_miss(c: &mut Criterion) {
    let registry = {
        let mut r = ToolRegistry::new();
        for i in 0..20 {
            let _ = r.insert(make_tool_fn(format!("tool_{i}")));
        }
        r
    };

    c.benchmark_group("tool_dispatch")
        .bench_function("lookup_missing_tool", |b| {
            b.iter(|| {
                let _ = registry.get(&ToolName::new("nonexistent"));
            })
        });
}

fn bench_definition_extraction(c: &mut Criterion) {
    let registry = {
        let mut r = ToolRegistry::new();
        for i in 0..20 {
            let _ = r.insert(make_tool_fn(format!("tool_{i}")));
        }
        r
    };

    c.benchmark_group("tool_dispatch")
        .bench_function("extract_all_definitions", |b| {
            b.iter(|| {
                let _ = registry.definitions();
            })
        });
}

criterion_group!(
    name = tool_dispatch;
    config = Criterion::default().sample_size(100);
    targets = bench_registry_insert, bench_registry_lookup, bench_registry_miss,
             bench_definition_extraction
);
criterion_main!(tool_dispatch);
