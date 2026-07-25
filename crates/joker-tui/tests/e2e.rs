//! End-to-end tests for the Joker TUI driver, covering the full agent run
//! lifecycle: construction → constraint file generation → agent run with
//! scripted model → event stream verification → completion.

use std::time::Duration;

use joker::SharedApprovalChannel;
use joker_config::RuntimeConfig;
use joker_tui::driver::AgentDriver;
use joker_tui::event::UiEvent;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Golden-path: driver constructs without error, constraint files are written,
/// a scripted run completes successfully with text delta and completion events.
#[tokio::test]
async fn e2e_golden_path_scripted_run() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let workspace = temp_dir.path().to_path_buf();

    // 1. Construct driver — this writes constraint files to .joker/agents/
    let driver = AgentDriver::new(
        RuntimeConfig {
            scripted_response: "Hello from Joker E2E test!".into(),
            ..RuntimeConfig::default()
        },
        workspace.clone(),
    );

    // 2. Verify constraint files were generated for built-in agents
    let agents_dir = workspace.join(".joker").join("agents");
    assert!(agents_dir.join("plan_agent.md").exists(), "plan_agent.md should exist");
    assert!(agents_dir.join("build_agent.md").exists(), "build_agent.md should exist");
    assert!(agents_dir.join("yolo_agent.md").exists(), "yolo_agent.md should exist");

    // 3. Verify the workspace is tracked correctly
    assert_eq!(driver.workspace(), &workspace);

    // 4. Spawn a run with scripted model
    let (tx, mut rx) = mpsc::unbounded_channel();
    let handle = driver
        .spawn_run(
            "Write a hello world program".into(),
            CancellationToken::new(),
            tx,
            SharedApprovalChannel::new(),
        )
        .expect("spawn_run should succeed");

    let mut saw_delta = false;
    let mut completed = false;

    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
        match event {
            UiEvent::Agent(joker::Event::ModelDelta { delta }) => {
                saw_delta |= delta.contains("Joker");
            }
            UiEvent::RunCompleted(result) => {
                assert!(result.is_ok(), "run should complete successfully: {result:?}");
                completed = true;
                break;
            }
            _ => {}
        }
    }

    handle.await.unwrap();
    assert!(saw_delta, "should receive text delta from scripted response");
    assert!(completed, "should receive RunCompleted event");
}

/// Verify that agent switching changes the active agent name in the driver.
#[tokio::test]
async fn e2e_agent_switching() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let workspace = temp_dir.path().to_path_buf();

    let mut driver = AgentDriver::new(
        RuntimeConfig::default(),
        workspace,
    );

    // Default active agent is "build"
    assert_eq!(driver.active_agent(), "build");

    // Switch to "plan" agent
    driver.set_active_agent("plan".into());
    assert_eq!(driver.active_agent(), "plan");

    // Switch to "yolo" agent
    driver.set_active_agent("yolo".into());
    assert_eq!(driver.active_agent(), "yolo");

    // Switch to unknown agent (should still accept it)
    driver.set_active_agent("custom-agent".into());
    assert_eq!(driver.active_agent(), "custom-agent");
}

/// Verify that the permission engine is accessible and has built-in profiles.
#[tokio::test]
async fn e2e_permission_engine_has_builtin_profiles() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let workspace = temp_dir.path().to_path_buf();

    let driver = AgentDriver::new(
        RuntimeConfig::default(),
        workspace,
    );

    let engine = driver.permission_engine();

    // Plan agent should deny mutating tools
    let decision = engine.evaluate("plan", &joker::ToolName::new("write_file"), true);
    assert!(matches!(decision, joker::PermissionDecision::Deny { .. }),
        "plan agent should hard-deny write_file: got {decision:?}");

    // Plan agent should allow read-only tools
    let decision = engine.evaluate("plan", &joker::ToolName::new("read_file"), false);
    assert_eq!(decision, joker::PermissionDecision::Allow,
        "plan agent should allow read_file");

    // Yolo agent should auto-accept write_file
    let decision = engine.evaluate("yolo", &joker::ToolName::new("write_file"), true);
    assert_eq!(decision, joker::PermissionDecision::Allow,
        "yolo agent should auto-accept write_file");
}

/// Verify that compact mode can be toggled on the driver.
#[tokio::test]
async fn e2e_compact_toggle() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let workspace = temp_dir.path().to_path_buf();

    let mut driver = AgentDriver::new(
        RuntimeConfig::default(),
        workspace,
    );

    // Default: compact not pending
    driver.set_compact_pending(true);
    driver.set_compact_pending(false);
    // Just verify no panic — compact state is a boolean toggle
}

/// Verify two concurrent scripted runs complete independently.
#[tokio::test]
async fn e2e_concurrent_runs() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let workspace = temp_dir.path().to_path_buf();

    let driver1 = AgentDriver::new(
        RuntimeConfig {
            scripted_response: "run A".into(),
            ..RuntimeConfig::default()
        },
        workspace.clone(),
    );
    let driver2 = AgentDriver::new(
        RuntimeConfig {
            scripted_response: "run B".into(),
            ..RuntimeConfig::default()
        },
        workspace,
    );

    let (tx1, mut rx1) = mpsc::unbounded_channel();
    let (tx2, mut rx2) = mpsc::unbounded_channel();

    let h1 = driver1
        .spawn_run("prompt A".into(), CancellationToken::new(), tx1, SharedApprovalChannel::new())
        .unwrap();
    let h2 = driver2
        .spawn_run("prompt B".into(), CancellationToken::new(), tx2, SharedApprovalChannel::new())
        .unwrap();

    let mut done = 0u8;
    while done < 2 {
        tokio::select! {
            Some(event) = rx1.recv() => {
                if matches!(event, UiEvent::RunCompleted(Ok(_))) {
                    done += 1;
                }
            }
            Some(event) = rx2.recv() => {
                if matches!(event, UiEvent::RunCompleted(Ok(_))) {
                    done += 1;
                }
            }
        }
    }

    h1.await.unwrap();
    h2.await.unwrap();
}

/// Verify that scripted model produces Finished event with correct stop reason.
#[tokio::test]
async fn e2e_scripted_model_stop_reason() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let workspace = temp_dir.path().to_path_buf();

    let driver = AgentDriver::new(
        RuntimeConfig {
            scripted_response: "test".into(),
            ..RuntimeConfig::default()
        },
        workspace,
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    let handle = driver
        .spawn_run(
            "test prompt".into(),
            CancellationToken::new(),
            tx,
            SharedApprovalChannel::new(),
        )
        .unwrap();

    let mut finished = false;
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await {
        if let UiEvent::Agent(joker::Event::ModelFinished { stop_reason, .. }) = event {
            assert_eq!(stop_reason, joker::StopReason::Stop);
            finished = true;
            break;
        }
    }

    handle.await.unwrap();
    assert!(finished, "should receive ModelFinished with StopReason::Stop");
}
