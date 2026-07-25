use std::time::Duration;

use joker::SharedApprovalChannel;
use joker_config::RuntimeConfig;
use joker_tui::driver::AgentDriver;
use joker_tui::event::UiEvent;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn scripted_driver_sends_agent_events_and_completion() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let driver = AgentDriver::new(
        RuntimeConfig {
            scripted_response: "hi from driver".into(),
            ..RuntimeConfig::default()
        },
        std::env::current_dir().unwrap(),
    );

    let handle = driver
        .spawn_run(
            "hello".into(),
            CancellationToken::new(),
            tx,
            SharedApprovalChannel::new(),
        )
        .unwrap();

    let mut saw_delta = false;
    let mut completed = false;
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await {
        match event {
            UiEvent::Agent(joker::Event::TextDelta { delta }) => {
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
async fn scripted_driver_completes_with_writeable_tools() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let driver = AgentDriver::new(
        RuntimeConfig {
            scripted_response: "done".into(),
            ..RuntimeConfig::default()
        },
        std::env::current_dir().unwrap(),
    );

    let handle = driver
        .spawn_run(
            "test".into(),
            CancellationToken::new(),
            tx,
            SharedApprovalChannel::new(),
        )
        .unwrap();

    let mut completed = false;
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await {
        if let UiEvent::RunCompleted(result) = event {
            assert!(result.is_ok(), "run failed: {result:?}");
            completed = true;
            break;
        }
    }

    handle.await.unwrap();
    assert!(completed);
}
