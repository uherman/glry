//! Fullscreen image viewer.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Paragraph;
use ratatui_image::{Resize, StatefulImage};

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
            let widget = StatefulImage::default().resize(Resize::Scale(None));
            f.render_stateful_widget(widget, area, &mut fill.proto);
            return;
        }
    }

    if let Some(proto) = app.fulls.get_mut(&img.path) {
        let target = app
            .full_images
            .get(&img.path)
            .map(|src| centered_fit_rect(area, src.width(), src.height(), app.picker.font_size()))
            .unwrap_or(area);
        let widget = StatefulImage::default();
        f.render_stateful_widget(widget, target, proto);
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

/// Sub-rect of `area` that the image will occupy under `Resize::Fit`,
/// centered so the letterbox margins are split evenly on both sides.
fn centered_fit_rect(area: Rect, img_w: u32, img_h: u32, font_size: (u16, u16)) -> Rect {
    let (fw, fh) = (font_size.0 as u32, font_size.1 as u32);
    if area.width == 0 || area.height == 0 || img_w == 0 || img_h == 0 || fw == 0 || fh == 0 {
        return area;
    }
    let avail_w = (area.width as u32) * fw;
    let avail_h = (area.height as u32) * fh;
    let target_w = avail_w.min(img_w);
    let target_h = avail_h.min(img_h);
    let wratio = target_w as f64 / img_w as f64;
    let hratio = target_h as f64 / img_h as f64;
    let ratio = f64::min(wratio, hratio);
    let fit_w_px = (img_w as f64 * ratio).round().max(1.0) as u32;
    let fit_h_px = (img_h as f64 * ratio).round().max(1.0) as u32;
    let cells_w = ((fit_w_px as f32 / fw as f32).ceil() as u16).min(area.width);
    let cells_h = ((fit_h_px as f32 / fh as f32).ceil() as u16).min(area.height);
    let x = area.x + (area.width - cells_w) / 2;
    let y = area.y + (area.height - cells_h) / 2;
    Rect::new(x, y, cells_w, cells_h)
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
