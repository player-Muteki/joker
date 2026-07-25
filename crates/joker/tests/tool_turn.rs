use std::sync::Arc;

use joker::{
    Agent, Content, DenyAllMutatingPolicy, ScriptedModel, ScriptedStep, StopReason,
    ToolAnnotations, ToolDefinition, ToolExecution, ToolFn, ToolFuture, ToolInvocation, ToolName,
    ToolOutput, ToolRegistry,
};
use serde_json::json;

fn echo(invocation: ToolInvocation) -> ToolFuture<'static> {
    Box::pin(async move { Ok(ToolOutput::new(invocation.arguments)) })
}

fn echo_tool(mutating: bool) -> ToolFn<fn(ToolInvocation) -> ToolFuture<'static>> {
    ToolFn::new(
        ToolDefinition {
            name: ToolName::new("echo"),
            description: "echo input".into(),
            input_schema: json!({"type":"object"}),
            annotations: ToolAnnotations {
                execution: ToolExecution::Sequential,
                mutating,
                timeout: None,
                ..ToolAnnotations::default()
            },
        },
        echo,
    )
}

#[tokio::test]
async fn tool_call_result_is_fed_back_before_final_answer() {
    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::tool_call("call-1", "echo", json!({"text":"hello"})),
        ScriptedStep::text("done"),
    ])) as Arc<dyn joker::Model>;
    let mut registry = ToolRegistry::new();
    registry.insert(echo_tool(false)).unwrap();
    let agent = Agent::new(model).with_tools(Arc::new(registry));

    let outcome = agent.run(joker::RunRequest::new("say it")).await.unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Stop);
    assert_eq!(outcome.conversation.messages().len(), 4);
    assert!(matches!(
        outcome.conversation.messages()[1].content[0],
        Content::ToolCall(_)
    ));
    let Content::ToolResult(result) = &outcome.conversation.messages()[2].content[0] else {
        panic!("expected tool result");
    };
    assert!(!result.is_error);
    assert_eq!(result.output, json!({"text":"hello"}));
    assert_eq!(
        outcome.conversation.messages()[3].content,
        vec![Content::text("done")]
    );
}

#[tokio::test]
async fn unknown_tool_becomes_error_result() {
    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::tool_call("call-1", "missing", json!({})),
        ScriptedStep::text("recovered"),
    ])) as Arc<dyn joker::Model>;
    let agent = Agent::new(model);

    let outcome = agent.run(joker::RunRequest::new("use tool")).await.unwrap();

    let Content::ToolResult(result) = &outcome.conversation.messages()[2].content[0] else {
        panic!("expected tool result");
    };
    assert!(result.is_error);
    assert!(result.output.as_str().unwrap().contains("unknown tool"));
}

#[tokio::test]
async fn policy_deny_becomes_error_result() {
    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::tool_call("call-1", "echo", json!({})),
        ScriptedStep::text("denied"),
    ])) as Arc<dyn joker::Model>;
    let mut registry = ToolRegistry::new();
    registry.insert(echo_tool(true)).unwrap();
    let agent = Agent::new(model)
        .with_tools(Arc::new(registry))
        .with_policy(Arc::new(DenyAllMutatingPolicy));

    let outcome = agent.run(joker::RunRequest::new("use tool")).await.unwrap();

    let Content::ToolResult(result) = &outcome.conversation.messages()[2].content[0] else {
        panic!("expected tool result");
    };
    assert!(result.is_error);
    assert!(result.output.as_str().unwrap().contains("denied"));
}
