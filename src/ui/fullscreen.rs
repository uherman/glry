//! Fullscreen image viewer.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
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
    let theme = app.theme;
    app.ensure_full(&img.path);
    if let Some(proto) = app.fulls.get_mut(&img.path) {
        let widget = StatefulImage::default();
        f.render_stateful_widget(widget, area, proto);
    } else if let Some(err) = app.errors.get(&img.path) {
        // Render the blank placeholder to properly clear the previous image
        // overlay (Kitty/Sixel) through the image protocol's own cleanup.
        let widget = StatefulImage::default();
        f.render_stateful_widget(widget, area, &mut app.loading_proto);
        let p = Paragraph::new(format!("\n  load error: {err}"))
            .style(Style::default().fg(theme.error_fg));
        f.render_widget(p, area);
    } else {
        // Render the blank placeholder to properly clear the previous image
        // overlay, then show a loading indicator on top.
        let widget = StatefulImage::default();
        f.render_stateful_widget(widget, area, &mut app.loading_proto);

        let label = format!("loading {}…", img.name);
        let y_offset = area.height / 2;
        if y_offset > 0 && area.height > y_offset {
            let inner = Rect::new(area.x, area.y + y_offset, area.width, 1);
            let p = Paragraph::new(label)
                .alignment(Alignment::Center)
                .style(
                    Style::default()
                        .fg(theme.loading_fg)
                        .add_modifier(Modifier::ITALIC),
                );
            f.render_widget(p, inner);
        }
    }
}
