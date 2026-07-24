use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem},
};

pub fn render(frame: &mut Frame<'_>, title: &str, options: &[(String, String)], selected: usize) {
    let area = frame.area();

    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Min(3),
            Constraint::Percentage(30),
        ])
        .split(area)[1];

    let horiz = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Min(20),
            Constraint::Percentage(25),
        ])
        .split(vert)[1];

    let list_items: Vec<ListItem> = options
        .iter()
        .map(|(label, _)| ListItem::new(Line::raw(label.as_str())))
        .collect();

    let list = List::new(list_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let mut list_state = ratatui::widgets::ListState::default();
    list_state.select(Some(selected));
    frame.render_stateful_widget(list, horiz, &mut list_state);
}
