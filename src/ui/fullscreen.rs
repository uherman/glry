//! Fullscreen image viewer.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Paragraph;
use ratatui_image::StatefulImage;

use crate::app::App;
use crate::scan::Entry;

pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
    let Some(idx) = app.fullscreen_idx else {
        return;
    };
    let Some(Entry::Image(img)) = app.entries.get(idx).cloned() else {
        return;
    };
    app.ensure_full(&img.path);
    if let Some(proto) = app.fulls.get_mut(&img.path) {
        let widget = StatefulImage::default();
        f.render_stateful_widget(widget, area, proto);
    } else if let Some(err) = app.errors.get(&img.path) {
        let p = Paragraph::new(format!("\n  load error: {err}"))
            .style(Style::default().fg(Color::Red));
        f.render_widget(p, area);
    } else {
        let p = Paragraph::new("\n  loading…").style(Style::default().fg(Color::DarkGray));
        f.render_widget(p, area);
    }
}
