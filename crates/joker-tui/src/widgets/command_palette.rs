use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::{app::App, commands};

pub fn render(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let suggestions = commands::suggestions(&app.composer);
    let lines = if suggestions.is_empty() {
        vec![Line::styled(
            "No matching commands",
            Style::default().fg(Color::DarkGray),
        )]
    } else {
        suggestions
            .into_iter()
            .map(|command| {
                Line::from(vec![
                    Span::styled(
                        command.usage,
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(command.description, Style::default().fg(Color::DarkGray)),
                ])
            })
            .collect()
    };

    let paragraph =
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Commands"));
    frame.render_widget(paragraph, area);
}
