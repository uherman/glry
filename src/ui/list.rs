//! List + preview view: filename list on the left, large preview on the right.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui_image::StatefulImage;

use crate::app::App;
use crate::scan::Entry;

pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(area);

    render_list(f, chunks[0], app);
    render_preview(f, chunks[1], app);
}

fn render_list(f: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .entries
        .iter()
        .map(|e| {
            let prefix = match e {
                Entry::Parent(_) => "..",
                Entry::SubDir { .. } => "📁",
                Entry::Image(_) => "🖼 ",
            };
            ListItem::new(format!(" {prefix}  {}", e.display_name()))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::RIGHT))
        .highlight_style(
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default();
    state.select(Some(app.selected));
    f.render_stateful_widget(list, area, &mut state);
}

fn render_preview(f: &mut Frame, area: Rect, app: &mut App) {
    let entry = match app.selected_entry().cloned() {
        Some(e) => e,
        None => return,
    };

    match entry {
        Entry::Parent(_) | Entry::SubDir { .. } => {
            let label = match entry {
                Entry::Parent(_) => "[ parent directory ]",
                Entry::SubDir { ref name, .. } => name,
                _ => unreachable!(),
            };
            let p = Paragraph::new(format!("\n\n  {label}"))
                .style(Style::default().fg(Color::Yellow));
            f.render_widget(p, area);
        }
        Entry::Image(img) => {
            app.ensure_full(&img.path);
            if let Some(proto) = app.fulls.get_mut(&img.path) {
                let widget = StatefulImage::default();
                f.render_stateful_widget(widget, area, proto);
            } else if let Some(err) = app.errors.get(&img.path) {
                let p = Paragraph::new(format!("\n  load error: {err}"))
                    .style(Style::default().fg(Color::Red));
                f.render_widget(p, area);
            } else {
                let p = Paragraph::new("\n  loading…")
                    .style(Style::default().fg(Color::DarkGray));
                f.render_widget(p, area);
            }
        }
    }
}
