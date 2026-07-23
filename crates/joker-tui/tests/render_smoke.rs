use joker_tui::app::{App, TranscriptItem};
use ratatui::{Terminal, backend::TestBackend};

#[test]
fn renders_main_layout_with_transcript_and_composer() {
    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = App::new();
    app.transcript.push(TranscriptItem::User("hello".into()));
    app.transcript.push(TranscriptItem::Assistant {
        text: "hi there".into(),
        streaming: false,
    });
    app.composer = "next".into();
    app.cursor = app.composer.len();

    terminal
        .draw(|frame| joker_tui::widgets::layout::render(frame, &app))
        .unwrap();

    let rendered = buffer_text(terminal.backend().buffer());
    assert!(rendered.contains("Joker"));
    assert!(rendered.contains("Idle"));
    assert!(rendered.contains("You: hello"));
    assert!(rendered.contains("Joker: hi there"));
    assert!(rendered.contains("next"));
}

fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
    buffer
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>()
}
