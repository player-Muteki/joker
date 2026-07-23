use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use crate::app::App;

pub fn render(frame: &mut Frame<'_>, area: Rect, app: &App) -> Position {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if app.running {
            Color::DarkGray
        } else {
            Color::White
        }))
        .title("Prompt");
    let inner = block.inner(area);
    let text = if app.running {
        app.composer.as_str()
    } else if app.composer.is_empty() {
        ""
    } else {
        app.composer.as_str()
    };

    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, area);

    let before_cursor = app
        .composer
        .get(..app.cursor)
        .unwrap_or(app.composer.as_str());
    let visible_width = UnicodeWidthStr::width(before_cursor) as u16;
    Position::new(
        inner
            .x
            .saturating_add(visible_width)
            .min(inner.right().saturating_sub(1)),
        inner.y,
    )
}
