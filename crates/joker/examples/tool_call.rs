use std::sync::Arc;

use joker::{
    Agent, RunRequest, ScriptedModel, ScriptedStep, ToolDefinition, ToolFn, ToolFuture,
    ToolInvocation, ToolName, ToolOutput, ToolRegistry,
};
use serde_json::json;

fn echo(invocation: ToolInvocation) -> ToolFuture<'static> {
    Box::pin(async move { Ok(ToolOutput::new(invocation.arguments)) })
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::tool_call("call-1", "echo", json!({"text":"hello"})),
        ScriptedStep::text("the echo tool returned hello"),
    ])) as Arc<dyn joker::Model>;

    let echo = ToolFn::new(
        ToolDefinition {
            name: ToolName::new("echo"),
            description: "returns its JSON arguments".into(),
            input_schema: json!({"type":"object"}),
            annotations: joker::ToolAnnotations::default(),
        },
        echo,
    );
    let mut tools = ToolRegistry::new();
    tools.insert(echo).unwrap();

    let agent = Agent::new(model).with_tools(Arc::new(tools));
    let outcome = agent.run(RunRequest::new("echo hello")).await.unwrap();
    println!("{:#?}", outcome.conversation.messages());
}
