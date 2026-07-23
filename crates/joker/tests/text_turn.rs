use std::sync::Arc;

use joker::{Agent, Content, RunRequest, ScriptedModel, ScriptedStep, StopReason};

#[tokio::test]
async fn text_turn_appends_user_and_final_assistant_message() {
    let model =
        Arc::new(ScriptedModel::new([ScriptedStep::text("hello")])) as Arc<dyn joker::Model>;
    let agent = Agent::new(model);

    let outcome = agent.run(RunRequest::new("hi")).await.unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Stop);
    assert_eq!(outcome.conversation.messages().len(), 2);
    assert_eq!(
        outcome.conversation.messages()[0],
        joker::Message::user("hi")
    );
    assert_eq!(
        outcome.conversation.messages()[1].role,
        joker::Role::Assistant
    );
    assert_eq!(
        outcome.conversation.messages()[1].content,
        vec![Content::text("hello")]
    );
}

#[tokio::test]
async fn model_error_terminates_run() {
    let model = Arc::new(ScriptedModel::new([ScriptedStep::Error("network".into())]))
        as Arc<dyn joker::Model>;
    let agent = Agent::new(model);

    let error = agent.run(RunRequest::new("hi")).await.unwrap_err();

    assert!(error.to_string().contains("network"));
}
