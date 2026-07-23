use std::time::Duration;

use joker_tui::driver::AgentDriver;
use joker_tui::event::UiEvent;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn scripted_driver_sends_agent_events_and_completion() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let driver = AgentDriver::new("hi from driver", false);

    let handle = driver
        .spawn_run("hello".into(), CancellationToken::new(), tx)
        .unwrap();

    let mut saw_delta = false;
    let mut completed = false;
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await {
        match event {
            UiEvent::Agent(joker::Event::ModelDelta { delta }) => {
                saw_delta |= delta.contains("hi");
            }
            UiEvent::RunCompleted(result) => {
                assert!(result.is_ok(), "run failed: {result:?}");
                completed = true;
                break;
            }
            _ => {}
        }
    }

    handle.await.unwrap();
    assert!(saw_delta);
    assert!(completed);
}

#[tokio::test]
async fn demo_tool_sends_tool_events() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let driver = AgentDriver::new("done", true);

    let handle = driver
        .spawn_run("echo me".into(), CancellationToken::new(), tx)
        .unwrap();

    let mut saw_tool_started = false;
    let mut saw_tool_finished = false;
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await {
        match event {
            UiEvent::Agent(joker::Event::ToolStarted { name, .. }) => {
                saw_tool_started |= name == "echo";
            }
            UiEvent::Agent(joker::Event::ToolFinished { result }) => {
                saw_tool_finished |= result.name == "echo" && !result.is_error;
            }
            UiEvent::RunCompleted(result) => {
                assert!(result.is_ok(), "run failed: {result:?}");
                break;
            }
            _ => {}
        }
    }

    handle.await.unwrap();
    assert!(saw_tool_started);
    assert!(saw_tool_finished);
}
