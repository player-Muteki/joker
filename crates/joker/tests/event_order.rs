use std::sync::Arc;

use joker::{Agent, Event, RecordingObserver, RunRequest, ScriptedModel, ScriptedStep, StopReason};

#[tokio::test]
async fn text_turn_event_order_is_stable() {
    let observer = RecordingObserver::new();
    let model =
        Arc::new(ScriptedModel::new([ScriptedStep::text("hello")])) as Arc<dyn joker::Model>;
    let agent = Agent::new(model).with_observer(Arc::new(observer.clone()));

    let outcome = agent.run(RunRequest::new("hi")).await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::Stop);

    let events = observer.events();
    assert!(matches!(events[0], Event::RunStarted));
    assert!(matches!(events[1], Event::ModelStarted));
    assert!(matches!(events[2], Event::ModelDelta { .. }));
    assert!(matches!(events[3], Event::ModelFinished { .. }));
    assert!(matches!(events[4], Event::RunFinished { .. }));
}

#[tokio::test]
async fn run_finished_is_emitted_on_model_error() {
    let observer = RecordingObserver::new();
    let model =
        Arc::new(ScriptedModel::new([ScriptedStep::Error("boom".into())])) as Arc<dyn joker::Model>;
    let agent = Agent::new(model).with_observer(Arc::new(observer.clone()));

    let error = agent.run(RunRequest::new("hi")).await.unwrap_err();
    assert!(error.to_string().contains("boom"));

    let events = observer.events();
    assert!(matches!(events.first(), Some(Event::RunStarted)));
    assert!(matches!(events.last(), Some(Event::RunFinished { .. })));
}
