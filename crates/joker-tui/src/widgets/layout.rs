use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::app::App;
use crate::widgets::{command_palette, composer, selector, transcript};

/// Render the top-level TUI layout: header, transcript, composer,
/// optional command palette, footer, and overlays.
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

    // API key input overlay
    if let Some((ref provider_id, ref input_buffer)) = app.api_key_input {
        let input_area = ratatui::layout::Rect {
            x: area.width.saturating_sub(60).min(area.width / 4),
            y: area.height.saturating_sub(6).min(area.height / 3),
            width: 60.min(area.width),
            height: 5.min(area.height),
        };
        let title = format!("Enter API key for {provider_id}:");
        let masked: String = input_buffer.chars().map(|c| if c.is_ascii_whitespace() { c } else { '*' }).collect();
        let block = ratatui::widgets::Block::default()
            .title(title)
            .borders(ratatui::widgets::Borders::ALL)
            .border_style(ratatui::style::Style::default().fg(ratatui::style::Color::Cyan));
        let paragraph = ratatui::widgets::Paragraph::new(masked)
            .block(block)
            .style(ratatui::style::Style::default().fg(ratatui::style::Color::White));
        frame.render_widget(paragraph, input_area);
    }

    if let Some(ref dialog) = app.dialog {
        selector::render(frame, &dialog.title, &dialog.options, dialog.selected);
    }
}
