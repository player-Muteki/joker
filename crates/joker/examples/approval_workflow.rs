use std::sync::Arc;

use joker::{
    AgentBuilder, ApprovalResponse, RunRequest, ScriptedModel, ScriptedStep, SharedApprovalChannel,
    ToolAnnotations, ToolDefinition, ToolFn, ToolFuture, ToolInvocation, ToolName, ToolOutput,
    ToolRegistry,
};
use serde_json::json;

fn write_tool() -> ToolFn<fn(ToolInvocation) -> ToolFuture<'static>> {
    ToolFn::new(
        ToolDefinition {
            name: ToolName::new("write_file"),
            description: "write content to a file".into(),
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
        ScriptedStep::tool_call("tc-1", "write_file", json!({"path":"test.txt"})),
        ScriptedStep::text("file written successfully"),
    ])) as Arc<dyn joker::Model>;

    let mut registry = ToolRegistry::new();
    registry.insert(write_tool()).unwrap();

    let channel = SharedApprovalChannel::new();
    let agent = AgentBuilder::new(model)
        .tools(Arc::new(registry))
        .approval_channel(channel.clone())
        .build();

    let handle = tokio::spawn(async move { agent.run(RunRequest::new("write test.txt")).await });

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    if let Some(req) = channel.pending_request() {
        println!("approval needed for: {} ({})", req.tool_name, req.reason);
        channel.respond(ApprovalResponse::Approved {
            remember_for_session: false,
        });
    }

    let outcome = handle.await.unwrap().unwrap();
    println!(
        "final message: {:?}",
        outcome.conversation.messages().last()
    );
}
