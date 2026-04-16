//! Fullscreen image viewer.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui_image::picker::Picker;
use ratatui_image::{Resize, ResizeEncodeRender, StatefulImage};

use crate::app::{AiState, Animation, App, FillProto};
use crate::scan::Entry;
use crate::thumbnail::center_crop_to_aspect;

pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
    let Some(idx) = app.fullscreen_idx else {
        return;
    };
    let Some(Entry::Image(img)) = app.entries.get(idx).cloned() else {
        return;
    };
    app.ensure_full(&img.path);
    render_image(f, area, app, &img);
    render_ai_overlay(f, area, app);
}

fn render_image(f: &mut Frame, area: Rect, app: &mut App, img: &crate::scan::ImageEntry) {
    let theme = app.theme;

    if app.fullscreen_crop {
        if let Some(anim) = app.animations.get_mut(&img.path) {
            ensure_anim_fill(anim, area, &app.picker);
            let target = anim.fill_target;
            if let Some(Some(proto)) = anim.fill_protos.get_mut(anim.current) {
                let widget = StatefulImage::default();
                f.render_stateful_widget(widget, target, proto);
                return;
            }
        } else {
            ensure_fill_proto(app, &img.path, area);
            if let Some(fill) = app.fill_proto.as_mut() {
                let widget = StatefulImage::default().resize(Resize::Scale(None));
                f.render_stateful_widget(widget, area, &mut fill.proto);
                return;
            }
        }
    }

    if let Some(anim) = app.animations.get_mut(&img.path) {
        ensure_anim_fit(anim, area, app.picker.font_size());
        let target = anim.fit_target;
        if let Some(proto) = anim.frames.get_mut(anim.current) {
            let widget = StatefulImage::default();
            f.render_stateful_widget(widget, target, proto);
        }
    } else if let Some(proto) = app.fulls.get_mut(&img.path) {
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
            let p = Paragraph::new(label).alignment(Alignment::Center).style(
                Style::default()
                    .fg(theme.loading_fg)
                    .add_modifier(Modifier::ITALIC),
            );
            f.render_widget(p, inner);
        }
    }
}

/// Render the AI-describe modal overlay, if one is open for the current
/// image. Uses ratatui's `Clear` so the terminal image protocol's pixels
/// don't bleed through the text box.
fn render_ai_overlay(f: &mut Frame, area: Rect, app: &App) {
    let Some(path) = app.ai_overlay.as_ref() else {
        return;
    };
    let state = app.ai_results.get(path);

    let (title, body, style) = match state {
        Some(AiState::AwaitingDecode) | Some(AiState::Requesting) => (
            " AI describe ",
            "Describing…".to_string(),
            Style::default()
                .fg(app.theme.loading_fg)
                .add_modifier(Modifier::ITALIC),
        ),
        Some(AiState::Ready(s)) => (
            " AI describe ",
            s.clone(),
            Style::default().fg(app.theme.status_fg),
        ),
        Some(AiState::Error(e)) => (
            " AI error ",
            e.clone(),
            Style::default().fg(app.theme.error_fg),
        ),
        None => return,
    };

    let modal = centered_modal_rect(area, 70, 50);
    if modal.width == 0 || modal.height == 0 {
        return;
    }
    f.render_widget(Clear, modal);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(app.theme.header_bg)
                .add_modifier(Modifier::BOLD),
        )
        .style(Style::default().bg(app.theme.status_bg));
    let hint = "\n\n  press a or Esc to close";
    let text = format!("{body}{hint}");
    let para = Paragraph::new(text)
        .block(block)
        .style(style)
        .wrap(Wrap { trim: true });
    f.render_widget(para, modal);
}

/// Centered modal sized to `cols` columns wide and `rows` rows tall (both
/// expressed as percentages of `area`, clamped to a sensible minimum so
/// the box doesn't collapse on small terminals).
fn centered_modal_rect(area: Rect, cols_pct: u16, rows_pct: u16) -> Rect {
    let w = ((area.width as u32 * cols_pct as u32 / 100) as u16)
        .max(20)
        .min(area.width);
    let h = ((area.height as u32 * rows_pct as u32 / 100) as u16)
        .max(6)
        .min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

/// Sub-rect of `area` that the image will occupy under `Resize::Fit`,
/// centered so the letterbox margins are split evenly on both sides.
pub(super) fn centered_fit_rect(area: Rect, img_w: u32, img_h: u32, font_size: (u16, u16)) -> Rect {
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

/// Make sure all fill-mode protocols are built and pre-encoded for `area`.
///
/// Each frame is cropped to the area's aspect, then encoded once at the
/// cropped image's natural size with `Resize::Fit` (no upscale-to-area).
/// Render uses `anim.fill_target` — the centered cell rect that fits the
/// cropped image — so the visible image still looks "filled" but the
/// per-frame upload stays bounded by the source-dim cap rather than blowing
/// up to terminal-pixel size.
fn ensure_anim_fill(anim: &mut Animation, area: Rect, picker: &Picker) {
    let (fw, fh) = picker.font_size();
    let aspect_w = (area.width as u32) * (fw as u32);
    let aspect_h = (area.height as u32) * (fh as u32);
    if aspect_w == 0 || aspect_h == 0 || anim.images.is_empty() {
        return;
    }
    let area_key = (area.width, area.height);
    let cached = anim.fill_area == Some(area_key) && anim.fill_protos.iter().all(|p| p.is_some());
    if cached {
        return;
    }

    let first_cropped = center_crop_to_aspect(anim.images[0].clone(), (aspect_w, aspect_h));
    let target = centered_fit_rect(
        area,
        first_cropped.width(),
        first_cropped.height(),
        (fw, fh),
    );

    anim.fill_area = Some(area_key);
    anim.fill_target = target;
    anim.fill_protos.clear();
    anim.fill_protos.reserve(anim.images.len());
    for (i, src) in anim.images.iter().enumerate() {
        let cropped = if i == 0 {
            first_cropped.clone()
        } else {
            center_crop_to_aspect(src.clone(), (aspect_w, aspect_h))
        };
        let mut proto = picker.new_resize_protocol(cropped);
        proto.resize_encode(&Resize::Fit(None), target);
        anim.fill_protos.push(Some(proto));
    }
}

/// Pre-encode every fit-mode frame protocol at the area's centered fit
/// rect, so frame advances during playback don't trigger lazy first-render
/// encoding (which causes the first-cycle flicker burst).
fn ensure_anim_fit(anim: &mut Animation, area: Rect, font_size: (u16, u16)) {
    if anim.frames.is_empty() {
        return;
    }
    let (w, h) = anim
        .images
        .first()
        .map(|src| (src.width(), src.height()))
        .unwrap_or(anim.original_dims);
    let target = centered_fit_rect(area, w, h, font_size);
    let area_key = (area.width, area.height);
    if anim.fit_area == Some(area_key) {
        return;
    }
    anim.fit_area = Some(area_key);
    anim.fit_target = target;
    for proto in anim.frames.iter_mut() {
        proto.resize_encode(&Resize::Fit(None), target);
    }
}
