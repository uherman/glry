//! Bottom status bar: per-entry metadata + transient status messages.

use chrono::{DateTime, Local};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::scan::{Entry, ImageEntry};

pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let text = if let Some(s) = &app.status {
        s.clone()
    } else {
        match app.selected_entry() {
            Some(Entry::Parent(_)) => " ..  (parent directory)".to_string(),
            Some(Entry::SubDir { name, .. }) => format!(" 📁 {name}  (directory)"),
            Some(Entry::Image(img)) => format_image(img, app),
            None => " (empty directory) ".to_string(),
        }
    };
    let p = Paragraph::new(text).style(Style::default().fg(Color::Gray).bg(Color::Black));
    f.render_widget(p, area);
}

fn format_image(img: &ImageEntry, app: &App) -> String {
    let dims = app
        .full_dims
        .get(&img.path)
        .map(|(w, h)| format!("  {w}×{h}"))
        .unwrap_or_default();
    let when: DateTime<Local> = img.modified.into();
    format!(
        " {}  {}{}  {}",
        img.name,
        human_size(img.size),
        dims,
        when.format("%Y-%m-%d %H:%M"),
    )
}

fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
