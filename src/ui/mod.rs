//! Top-level UI rendering. Dispatches to the right view based on app state.

mod fullscreen;
mod grid;
mod info;
mod list;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Paragraph;

use crate::app::{App, ViewMode};

pub fn render(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(1),    // main
            Constraint::Length(1), // footer / file info
        ])
        .split(f.area());

    render_header(f, chunks[0], app);

    if app.fullscreen_idx.is_some() {
        fullscreen::render(f, chunks[1], app);
    } else {
        match app.view {
            ViewMode::Grid => grid::render(f, chunks[1], app),
            ViewMode::List => list::render(f, chunks[1], app),
        }
    }

    info::render(f, chunks[2], app);
}

fn render_header(f: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let mode = if app.fullscreen_idx.is_some() {
        "FULL"
    } else {
        match app.view {
            ViewMode::Grid => "GRID",
            ViewMode::List => "LIST",
        }
    };
    let total_images = app
        .entries
        .iter()
        .filter(|e| matches!(e, crate::scan::Entry::Image(_)))
        .count();
    let line = format!(
        " glry  [{}]  {}   {} images   {}/{} ",
        mode,
        app.cwd.display(),
        total_images,
        app.selected.saturating_add(1),
        app.entries.len(),
    );
    let p = Paragraph::new(line).style(
        Style::default()
            .fg(app.theme.header_fg)
            .bg(app.theme.header_bg)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(p, area);
}
