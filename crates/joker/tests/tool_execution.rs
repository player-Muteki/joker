use std::{sync::Arc, time::Duration};

use joker::{
    Agent, AgentConfig, Content, Event, ExecutionMode, RecordingObserver, ScriptedModel,
    ScriptedStep, ToolAnnotations, ToolDefinition, ToolError, ToolExecution, ToolFn, ToolFuture,
    ToolInvocation, ToolName, ToolOutput, ToolRegistry,
};
use serde_json::json;

fn slow_first(invocation: ToolInvocation) -> ToolFuture<'static> {
    Box::pin(async move {
        tokio::time::sleep(Duration::from_millis(40)).await;
        Ok(ToolOutput::new(json!({ "name": invocation.name.as_str() })))
    })
}

fn fast_second(invocation: ToolInvocation) -> ToolFuture<'static> {
    Box::pin(async move {
        tokio::time::sleep(Duration::from_millis(5)).await;
        Ok(ToolOutput::new(json!({ "name": invocation.name.as_str() })))
    })
}

fn make_tool(
    name: &'static str,
    handler: fn(ToolInvocation) -> ToolFuture<'static>,
) -> ToolFn<fn(ToolInvocation) -> ToolFuture<'static>> {
    ToolFn::new(
        ToolDefinition {
            name: ToolName::new(name),
            description: name.into(),
            input_schema: json!({"type":"object"}),
            annotations: ToolAnnotations {
                execution: ToolExecution::ParallelSafe,
                mutating: false,
                timeout: None,
            },
        },
        handler,
    )
}

#[tokio::test]
async fn parallel_finish_events_can_differ_from_transcript_order() {
    let observer = RecordingObserver::new();
    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::message(
            vec![
                Content::ToolCall(joker::ToolCall {
                    id: "call-1".into(),
                    name: "slow".into(),
                    arguments: json!({}),
                }),
                Content::ToolCall(joker::ToolCall {
                    id: "call-2".into(),
                    name: "fast".into(),
                    arguments: json!({}),
                }),
            ],
            joker::StopReason::ToolUse,
        ),
        ScriptedStep::text("done"),
    ])) as Arc<dyn joker::Model>;
    let mut registry = ToolRegistry::new();
    registry.insert(make_tool("slow", slow_first)).unwrap();
    registry.insert(make_tool("fast", fast_second)).unwrap();
    let agent = Agent::new(model)
        .with_tools(Arc::new(registry))
        .with_observer(Arc::new(observer.clone()))
        .with_config(AgentConfig {
            execution_mode: ExecutionMode::ParallelWhenSafe,
            ..AgentConfig::default()
        });

    let outcome = agent.run(joker::RunRequest::new("parallel")).await.unwrap();

    assert_eq!(outcome.conversation.messages()[2].content.len(), 2);
    let Content::ToolResult(first) = &outcome.conversation.messages()[2].content[0] else {
        panic!("expected first result");
    };
    let Content::ToolResult(second) = &outcome.conversation.messages()[2].content[1] else {
        panic!("expected second result");
    };
    assert_eq!(first.name, "slow");
    assert_eq!(second.name, "fast");

    let finished_names: Vec<_> = observer
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::ToolFinished { result } => Some(result.name),
            _ => None,
        })
        .collect();
    assert_eq!(finished_names, vec!["fast", "slow"]);
}

#[tokio::test]
async fn tool_timeout_becomes_error_result() {
    fn timeout_tool(_invocation: ToolInvocation) -> ToolFuture<'static> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(ToolOutput::new(json!("late")))
        })
    }

    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::tool_call("call-1", "timeout", json!({})),
        ScriptedStep::text("done"),
    ])) as Arc<dyn joker::Model>;
    let mut registry = ToolRegistry::new();
    registry
        .insert(ToolFn::new(
            ToolDefinition {
                name: ToolName::new("timeout"),
                description: "times out".into(),
                input_schema: json!({"type":"object"}),
                annotations: ToolAnnotations {
                    execution: ToolExecution::Sequential,
                    mutating: false,
                    timeout: Some(Duration::from_millis(1)),
                },
            },
            timeout_tool as fn(ToolInvocation) -> ToolFuture<'static>,
        ))
        .unwrap();
    let agent = Agent::new(model).with_tools(Arc::new(registry));

    let outcome = agent.run(joker::RunRequest::new("timeout")).await.unwrap();

    let Content::ToolResult(result) = &outcome.conversation.messages()[2].content[0] else {
        panic!("expected tool result");
    };
    assert!(result.is_error);
    assert!(result.output.as_str().unwrap().contains("timed out"));
}

#[tokio::test]
async fn invalid_arguments_and_tool_failures_become_error_results() {
    fn invalid(_invocation: ToolInvocation) -> ToolFuture<'static> {
        Box::pin(async move { Err(ToolError::InvalidArguments("missing field".into())) })
    }

    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::tool_call("call-1", "invalid", json!({})),
        ScriptedStep::text("recovered"),
    ])) as Arc<dyn joker::Model>;
    let mut registry = ToolRegistry::new();
    registry
        .insert(ToolFn::new(
            ToolDefinition {
                name: ToolName::new("invalid"),
                description: "validates args".into(),
                input_schema: json!({"type":"object"}),
                annotations: ToolAnnotations::default(),
            },
            invalid as fn(ToolInvocation) -> ToolFuture<'static>,
        ))
        .unwrap();
    let agent = Agent::new(model).with_tools(Arc::new(registry));

    let outcome = agent.run(joker::RunRequest::new("invalid")).await.unwrap();

    let Content::ToolResult(result) = &outcome.conversation.messages()[2].content[0] else {
        panic!("expected tool result");
    };
    assert!(result.is_error);
    assert!(
        result
            .output
            .as_str()
            .unwrap()
            .contains("invalid arguments")
    );
}


// ── PermissionPolicy + ApprovalChannel integration ────────────────────

use std::sync::atomic::{AtomicUsize, Ordering};

/// A model that returns a tool call on first stream() call, then text on subsequent calls.
struct ToolThenTextModel {
    tool_name: String,
    response_text: String,
    call_count: AtomicUsize,
}

impl ToolThenTextModel {
    fn new(tool_name: impl Into<String>, response_text: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
            response_text: response_text.into(),
            call_count: AtomicUsize::new(0),
        }
    }
}

impl joker::Model for ToolThenTextModel {
    fn stream(&self, _request: joker::ModelRequest) -> joker::ModelFuture<'_> {
        let tool_name = self.tool_name.clone();
        let response_text = self.response_text.clone();
        let call = self.call_count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if call == 0 {
                // First call: return tool call
                let items: Vec<Result<_, _>> = vec![
                    Ok(joker::ModelResponseEvent::ToolCall(joker::ToolCall {
                        id: "tc-1".into(),
                        name: tool_name,
                        arguments: serde_json::json!({}),
                    })),
                    Ok(joker::ModelResponseEvent::Finished {
                        stop_reason: joker::StopReason::ToolUse,
                        usage: joker::Usage::default(),
                    }),
                ];
                Ok(Box::new(futures_util::stream::iter(items)) as joker::ModelStream)
            } else {
                // Subsequent calls: return text
                let items: Vec<Result<_, _>> = vec![
                    Ok(joker::ModelResponseEvent::TextDelta(response_text)),
                    Ok(joker::ModelResponseEvent::Finished {
                        stop_reason: joker::StopReason::Stop,
                        usage: joker::Usage::default(),
                    }),
                ];
                Ok(Box::new(futures_util::stream::iter(items)) as joker::ModelStream)
            }
        })
    }
}

#[tokio::test]
async fn permission_ask_pauses_for_approval_before_mutating_tool() {
    let model = Arc::new(ToolThenTextModel::new("write_file", "File written."));

    let mut registry = joker::ToolRegistry::new();
    registry
        .insert(joker::ToolFn::new(
            joker::ToolDefinition {
                name: joker::ToolName::new("write_file"),
                description: "write a file".into(),
                input_schema: serde_json::json!({}),
                annotations: joker::ToolAnnotations {
                    execution: joker::ToolExecution::Sequential,
                    mutating: true,
                    timeout: None,
                },
            },
            |_invocation: joker::ToolInvocation| -> joker::ToolFuture<'static> {
                Box::pin(async move {
                    Ok(joker::ToolOutput::new(serde_json::json!({"ok": true})))
                })
            },
        ))
        .unwrap();

    let policy = Arc::new(
        joker::PermissionPolicy::new()
            .with_default_for_mutating(joker::ToolDecision::Ask {
                request_id: "write_file-0".into(),
                reason: "mutating tool needs approval".into(),
            }),
    );

    let approval_channel = joker::SharedApprovalChannel::new();
    let channel_for_agent = approval_channel.clone();

    let agent = joker::AgentBuilder::new(model)
        .tools(Arc::new(registry))
        .permissions(policy)
        .approval_channel(channel_for_agent)
        .build();

    let run_handle = tokio::spawn(async move {
        agent.run(joker::RunRequest::new("write a file")).await
    });

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Verify a pending approval request was submitted
    let pending = approval_channel.pending_request();
    assert!(pending.is_some(), "agent should have submitted an approval request");
    let req = pending.unwrap();
    assert_eq!(req.request_id, "ask-write_file");
    assert_eq!(req.tool_name, "write_file");

    // Respond with approval
    approval_channel.respond(joker::ApprovalResponse::Approved {
        remember_for_session: false,
    });

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_handle,
    )
    .await
    .expect("run should complete within timeout")
    .expect("run should not panic");

    assert!(outcome.is_ok(), "run should succeed after approval: {:?}", outcome);
    let outcome = outcome.unwrap();
    assert_eq!(outcome.stop_reason, joker::StopReason::Stop);
}

#[tokio::test]
async fn permission_ask_is_deniable() {
    let model = Arc::new(ToolThenTextModel::new("shell", "Skipped."));

    let mut registry = joker::ToolRegistry::new();
    registry
        .insert(joker::ToolFn::new(
            joker::ToolDefinition {
                name: joker::ToolName::new("shell"),
                description: "run a command".into(),
                input_schema: serde_json::json!({}),
                annotations: joker::ToolAnnotations {
                    execution: joker::ToolExecution::Sequential,
                    mutating: true,
                    timeout: None,
                },
            },
            |_invocation: joker::ToolInvocation| -> joker::ToolFuture<'static> {
                Box::pin(async move {
                    Ok(joker::ToolOutput::new(serde_json::json!({"done": true})))
                })
            },
        ))
        .unwrap();

    let policy = Arc::new(
        joker::PermissionPolicy::new()
            .with_default_for_mutating(joker::ToolDecision::Ask {
                request_id: "shell-ask".into(),
                reason: "needs approval".into(),
            }),
    );

    let approval_channel = joker::SharedApprovalChannel::new();
    let chan = approval_channel.clone();

    let agent = joker::AgentBuilder::new(model)
        .tools(Arc::new(registry))
        .permissions(policy)
        .approval_channel(chan)
        .build();

    let run_handle = tokio::spawn(async move {
        agent.run(joker::RunRequest::new("run a command")).await
    });

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let pending = approval_channel.pending_request();
    assert!(pending.is_some(), "agent should have submitted an approval request");

    // Deny the request
    approval_channel.respond(joker::ApprovalResponse::Denied {
        reason: "not now".into(),
    });

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_handle,
    )
    .await
    .expect("run should complete")
    .expect("run should not panic");

    assert!(outcome.is_ok(), "run should succeed even after denial");
    let conversation = outcome.unwrap().conversation;
    // There should be a tool result with is_error = true
    let has_denied_tool = conversation.messages().iter().any(|msg| {
        msg.content.iter().any(|c| {
            if let joker::Content::ToolResult(result) = c {
                result.is_error
            } else {
                false
            }
        })
    });
    assert!(has_denied_tool, "denied tool should produce an error result");
}
