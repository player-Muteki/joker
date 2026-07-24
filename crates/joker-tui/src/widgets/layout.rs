use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::app::App;
use crate::widgets::{command_palette, composer, selector, transcript};

pub fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let show_commands = app.composer.starts_with('/');
    let constraints = if show_commands {
        vec![
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(6),
            Constraint::Length(3),
            Constraint::Length(1),
        ]
    } else {
        vec![
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ]
    };
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
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
    let composer_index = if show_commands {
        command_palette::render(frame, vertical[2], app);
        3
    } else {
        2
    };
    let cursor = composer::render(frame, vertical[composer_index], app);
    frame.set_cursor_position(cursor);

    let footer_text = if app.running {
        "Esc/Ctrl-C cancel"
    } else if app.dialog.is_some() {
        "↑↓ navigate | Enter select | Esc cancel"
    } else {
        "Enter submit | Esc/Ctrl-C quit"
    };
    let footer = Paragraph::new(footer_text).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(footer, vertical[composer_index + 1]);

    if let Some(ref dialog) = app.dialog {
        selector::render(frame, &dialog.title, &dialog.options, dialog.selected);
    }
}
