use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::app::{App, ToolState, TranscriptItem};

pub fn render(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let lines = transcript_lines(app);
    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::BOTTOM))
        .scroll((app.scroll, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn transcript_lines(app: &App) -> Vec<Line<'static>> {
    app.transcript
        .iter()
        .flat_map(item_lines)
        .collect::<Vec<_>>()
}

fn item_lines(item: &TranscriptItem) -> Vec<Line<'static>> {
    match item {
        TranscriptItem::User(text) => labeled_lines("You", Color::Cyan, text),
        TranscriptItem::Assistant { text, streaming } => {
            let label = if *streaming { "Joker *" } else { "Joker" };
            labeled_lines(label, Color::Magenta, text)
        }
        TranscriptItem::Tool {
            call_id,
            name,
            state,
        } => {
            let (marker, color, detail) = match state {
                ToolState::Running => ("running", Color::Yellow, String::new()),
                ToolState::Done(output) => ("ok", Color::Green, summarize_value(output)),
                ToolState::Error(output) => ("error", Color::Red, summarize_value(output)),
            };
            vec![Line::from(vec![
                Span::styled(
                    "Tool",
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" {name} [{call_id}] ")),
                Span::styled(marker, Style::default().fg(color)),
                Span::raw(detail),
            ])]
        }
        TranscriptItem::Status(text) => {
            vec![Line::styled(
                text.clone(),
                Style::default().fg(Color::DarkGray),
            )]
        }
        TranscriptItem::Error(text) => {
            vec![Line::styled(
                format!("Error: {text}"),
                Style::default().fg(Color::Red),
            )]
        }
        TranscriptItem::ApprovalRequest {
            request_id,
            tool_name,
            subject,
            reason,
        } => {
            let subject_display = if subject.is_empty() {
                String::new()
            } else {
                format!(" ({subject})")
            };
            vec![
                Line::from(vec![
                    Span::styled(
                        "🔐 Approval Needed",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(" [{request_id}]")),
                ]),
                Line::from(vec![
                    Span::styled("  Tool: ", Style::default().fg(Color::Blue)),
                    Span::raw(format!("{tool_name}{subject_display}")),
                ]),
                Line::from(vec![
                    Span::styled("  Reason: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(reason.clone()),
                ]),
                Line::from(vec![Span::styled(
                    "  → Use /approve <id> or /deny <id> [reason]",
                    Style::default().fg(Color::DarkGray),
                )]),
            ]
        }
    }
}

fn labeled_lines(label: &'static str, color: Color, text: &str) -> Vec<Line<'static>> {
    if text.is_empty() {
        return vec![Line::from(vec![Span::styled(
            label,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )])];
    }

    text.lines()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                Line::from(vec![
                    Span::styled(
                        format!("{label}: "),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(line.to_string()),
                ])
            } else {
                Line::from(vec![Span::raw(format!("  {line}"))])
            }
        })
        .collect()
}

fn summarize_value(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        format!(" {value}")
    }
}
