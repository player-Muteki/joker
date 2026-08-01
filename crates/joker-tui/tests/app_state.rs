use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use joker::{ApprovalResponse, SharedApprovalChannel};
use joker_tui::app::{App, AppAction, ToolState, TranscriptItem};
use joker_tui::event::UiEvent;
use serde_json::json;
use tokio::sync::mpsc;

#[test]
fn edits_and_submits_prompt() {
    let mut app = App::new();

    app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    assert_eq!(app.composer, "hi");
    assert_eq!(app.cursor, 2);

    assert_eq!(
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
        Some(AppAction::Redraw)
    );
    assert_eq!(app.composer, "h");

    assert_eq!(
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(AppAction::Submit("h".into()))
    );
    assert!(app.running);
    assert_eq!(app.composer, "");
    assert_eq!(
        app.transcript.last(),
        Some(&TranscriptItem::User("h".into()))
    );
}

#[test]
fn unicode_editing_uses_char_boundaries() {
    let mut app = App::new();

    app.handle_key(KeyEvent::new(KeyCode::Char('你'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('好'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

    assert_eq!(app.composer, "好");
}

#[test]
fn applies_text_delta_to_streaming_assistant_item() {
    let mut app = App::new();

    app.apply_ui_event(UiEvent::Agent(joker::Event::ModelStarted));
    app.apply_ui_event(UiEvent::Agent(joker::Event::TextDelta {
        delta: "hel".into(),
    }));
    app.apply_ui_event(UiEvent::Agent(joker::Event::TextDelta {
        delta: "lo".into(),
    }));
    app.apply_ui_event(UiEvent::Agent(joker::Event::ModelFinished {
        stop_reason: joker::StopReason::Stop,
    }));

    assert_eq!(
        app.transcript.last(),
        Some(&TranscriptItem::Assistant {
            text: "hello".into(),
            streaming: false
        })
    );
}

#[test]
fn applies_tool_result_to_existing_tool_item() {
    let mut app = App::new();

    app.apply_ui_event(UiEvent::Agent(joker::Event::ToolStarted {
        call_id: "call-1".into(),
        name: "echo".into(),
    }));
    app.apply_ui_event(UiEvent::Agent(joker::Event::ToolFinished {
        result: joker::ToolResult::ok("call-1", "echo", json!({"echo": "hi"})),
    }));

    assert_eq!(
        app.transcript.last(),
        Some(&TranscriptItem::Tool {
            call_id: "call-1".into(),
            name: "echo".into(),
            state: ToolState::Done(r#"{"echo":"hi"}"#.into())
        })
    );
}

#[test]
fn approvals_respond_through_channel_even_with_runtime_handle() {
    let mut app = App::new();
    let channel = SharedApprovalChannel::new();
    let (tx, _rx) = mpsc::unbounded_channel();
    app.approval_channel = Some(channel.clone());
    app.runtime_handle = Some(joker::AgentRuntimeHandle::new(tx));

    app.approve_pending("req-1", true);
    assert!(matches!(
        channel.take_response(),
        Some(ApprovalResponse::Approved {
            remember_for_session: true
        })
    ));

    app.deny_pending("req-2", Some("no"));
    assert!(matches!(
        channel.take_response(),
        Some(ApprovalResponse::Denied { reason }) if reason == "no"
    ));
}

#[test]
fn escape_quits_when_idle_and_cancels_when_running() {
    let mut app = App::new();
    assert_eq!(
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Some(AppAction::Quit)
    );

    app.running = true;
    assert_eq!(
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Some(AppAction::Cancel)
    );
}
