use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::app::App;
use crate::widgets::{composer, transcript};

pub fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    let status_style = if app.running {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Green)
    };
    let header = Paragraph::new(Line::from(vec![
        Span::styled("Joker", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(app.status.as_str(), status_style),
    ]))
    .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(header, vertical[0]);

    transcript::render(frame, vertical[1], app);
    let cursor = composer::render(frame, vertical[2], app);
    frame.set_cursor_position(cursor);

    let footer_text = if app.running {
        "Esc/Ctrl-C cancel"
    } else {
        "Enter submit | Esc/Ctrl-C quit"
    };
    let footer = Paragraph::new(footer_text).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(footer, vertical[3]);
}
