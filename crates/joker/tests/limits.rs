use std::sync::Arc;

use joker::{Agent, AgentConfig, RunLimits, ScriptedModel, ScriptedStep, StopReason};
use serde_json::json;

#[tokio::test]
async fn max_steps_prevents_infinite_tool_loop() {
    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::tool_call("call-1", "missing", json!({})),
        ScriptedStep::tool_call("call-2", "missing", json!({})),
    ])) as Arc<dyn joker::Model>;
    let config = AgentConfig {
        limits: RunLimits {
            max_steps: 1,
            max_tool_calls: 64,
        },
        ..AgentConfig::default()
    };
    let agent = Agent::new(model).with_config(config);

    let outcome = agent.run(joker::RunRequest::new("loop")).await.unwrap();

    assert_eq!(outcome.stop_reason, StopReason::LimitReached);
}

#[tokio::test]
async fn max_tool_calls_stops_before_executing_over_limit_batch() {
    let model = Arc::new(ScriptedModel::new([ScriptedStep::tool_call(
        "call-1",
        "missing",
        json!({}),
    )])) as Arc<dyn joker::Model>;
    let config = AgentConfig {
        limits: RunLimits {
            max_steps: 16,
            max_tool_calls: 0,
        },
        ..AgentConfig::default()
    };
    let agent = Agent::new(model).with_config(config);

    let outcome = agent.run(joker::RunRequest::new("loop")).await.unwrap();

    assert_eq!(outcome.stop_reason, StopReason::LimitReached);
    assert_eq!(outcome.conversation.messages().len(), 2);
}
