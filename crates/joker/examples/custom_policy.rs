use std::sync::Arc;

use joker::{
    Agent, AgentBuilder, RunRequest, ScriptedModel, ScriptedStep, Tool, ToolAnnotations,
    ToolDecision, ToolDefinition, ToolFn, ToolFuture, ToolInvocation, ToolName, ToolOutput,
    ToolPolicy, ToolPolicyRequest, ToolRegistry,
};
use serde_json::json;

struct AllowOnlyReadPolicy;

impl ToolPolicy for AllowOnlyReadPolicy {
    fn evaluate(&self, request: ToolPolicyRequest<'_>) -> joker::PolicyFuture<'_> {
        let allowed = ["read_file", "list_files", "grep", "glob"];
        let name = request.invocation.name.clone();
        Box::pin(async move {
            if allowed.contains(&name.as_str()) {
                Ok(ToolDecision::Allow)
            } else {
                Ok(ToolDecision::Deny {
                    reason: "only read-only tools allowed by policy".into(),
                })
            }
        })
    }
}

fn read_file() -> ToolFn<fn(ToolInvocation) -> ToolFuture<'static>> {
    ToolFn::new(
        ToolDefinition {
            name: ToolName::new("read_file"),
            description: "read a file".into(),
            input_schema: json!({"type":"object"}),
            annotations: ToolAnnotations {
                mutating: false,
                ..ToolAnnotations::default()
            },
        },
        |_: ToolInvocation| -> ToolFuture<'static> {
            Box::pin(async move { Ok(ToolOutput::new(json!({"content": "file content"}))) })
        },
    )
}

fn write_file() -> ToolFn<fn(ToolInvocation) -> ToolFuture<'static>> {
    ToolFn::new(
        ToolDefinition {
            name: ToolName::new("write_file"),
            description: "write a file".into(),
            input_schema: json!({"type":"object"}),
            annotations: ToolAnnotations {
                mutating: true,
                ..ToolAnnotations::default()
            },
        },
        |_: ToolInvocation| -> ToolFuture<'static> {
            Box::pin(async move { Ok(ToolOutput::new(json!({"ok": true}))) })
        },
    )
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::tool_call("c1", "read_file", json!({"path":"x.txt"})),
        ScriptedStep::text("read file ok"),
    ])) as Arc<dyn joker::Model>;

    let mut registry = ToolRegistry::new();
    registry.insert(read_file()).unwrap();
    registry.insert(write_file()).unwrap();

    let agent = AgentBuilder::new(model)
        .tools(Arc::new(registry))
        .permissions(Arc::new(AllowOnlyReadPolicy))
        .build();

    let outcome = agent.run(RunRequest::new("read x.txt")).await.unwrap();
    for msg in outcome.conversation.messages() {
        println!("{:?}: {:?}", msg.role, msg.content);
    }
}
