use std::sync::Arc;

use joker::{Agent, RunError, RunRequest, ScriptedModel, ScriptedStep};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn cancellation_before_run_does_not_retry() {
    let model =
        Arc::new(ScriptedModel::new([ScriptedStep::text("unreachable")])) as Arc<dyn joker::Model>;
    let agent = Agent::new(model);
    let token = CancellationToken::new();
    token.cancel();

    let error = agent
        .run(RunRequest::new("hi").with_cancellation_token(token))
        .await
        .unwrap_err();

    assert!(matches!(error, RunError::Cancelled));
}

#[tokio::test]
async fn model_cancellation_surfaces_as_run_error() {
    let model = Arc::new(ScriptedModel::new([ScriptedStep::Cancelled])) as Arc<dyn joker::Model>;
    let agent = Agent::new(model);

    let error = agent.run(RunRequest::new("hi")).await.unwrap_err();

    assert!(error.to_string().contains("cancelled"));
}
