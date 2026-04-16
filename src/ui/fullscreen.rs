//! Fullscreen image viewer.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Paragraph;
use ratatui_image::StatefulImage;

use crate::app::{App, FillProto};
use crate::scan::Entry;
use crate::thumbnail::center_crop_to_aspect;

pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
    let Some(idx) = app.fullscreen_idx else {
        return;
    };
    let Some(Entry::Image(img)) = app.entries.get(idx).cloned() else {
        return;
    };
    let theme = app.theme;
    app.ensure_full(&img.path);

    if app.fullscreen_crop {
        ensure_fill_proto(app, &img.path, area);
        if let Some(fill) = app.fill_proto.as_mut() {
            let widget = StatefulImage::default();
            f.render_stateful_widget(widget, area, &mut fill.proto);
            return;
        }
    }

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

/// Rebuild `app.fill_proto` if it's missing, or doesn't match the current
/// path/area. Does nothing if the image isn't decoded yet.
fn ensure_fill_proto(app: &mut App, path: &std::path::Path, area: Rect) {
    let matches = app
        .fill_proto
        .as_ref()
        .is_some_and(|f| f.path == path && f.area_w == area.width && f.area_h == area.height);
    if matches {
        return;
    }
    let Some(src) = app.full_images.get(path) else {
        app.fill_proto = None;
        return;
    };
    let (fw, fh) = app.picker.font_size();
    let aspect_w = (area.width as u32) * (fw as u32);
    let aspect_h = (area.height as u32) * (fh as u32);
    if aspect_w == 0 || aspect_h == 0 {
        return;
    }
    let cropped = center_crop_to_aspect(src.clone(), (aspect_w, aspect_h));
    let proto = app.picker.new_resize_protocol(cropped);
    app.fill_proto = Some(FillProto {
        path: path.to_path_buf(),
        area_w: area.width,
        area_h: area.height,
        proto,
    });
}
