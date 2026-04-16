//! List + preview view: filename list on the left, large preview on the right.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui_image::picker::Picker;
use ratatui_image::{Resize, ResizeEncodeRender, StatefulImage};

use crate::app::{Animation, App};
use crate::scan::Entry;
use crate::ui::fullscreen::centered_fit_rect;

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
                .bg(app.theme.selection_bg)
                .fg(app.theme.selection_fg)
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
                .style(Style::default().fg(app.theme.directory_fg));
            f.render_widget(p, area);
        }
        Entry::Image(img) => {
            let theme = app.theme;
            app.ensure_full(&img.path);
            if let Some(anim) = app.animations.get_mut(&img.path) {
                ensure_anim_preview(anim, area, &app.picker);
                let target = anim.preview_target;
                if let Some(proto) = anim.preview_proto.as_mut() {
                    let widget = StatefulImage::default();
                    f.render_stateful_widget(widget, target, proto);
                    return;
                }
            }
            if let Some(proto) = app.fulls.get_mut(&img.path) {
                let widget = StatefulImage::default();
                f.render_stateful_widget(widget, area, proto);
            } else if let Some(err) = app.errors.get(&img.path) {
                let p = Paragraph::new(format!("\n  load error: {err}"))
                    .style(Style::default().fg(theme.error_fg));
                f.render_widget(p, area);
            } else {
                let p = Paragraph::new("\n  loading…")
                    .style(Style::default().fg(theme.loading_fg));
                f.render_widget(p, area);
            }
        }
    }
}

/// Build (and pre-encode) a static first-frame protocol for the list-view
/// preview if the cache is missing or sized for a different area. Kept
/// separate from the fullscreen caches so toggling between the two modes
/// doesn't re-encode every frame on each switch.
fn ensure_anim_preview(anim: &mut Animation, area: Rect, picker: &Picker) {
    if anim.images.is_empty() {
        return;
    }
    let area_key = (area.width, area.height);
    if anim.preview_area == Some(area_key) && anim.preview_proto.is_some() {
        return;
    }
    let src = &anim.images[0];
    let target = centered_fit_rect(area, src.width(), src.height(), picker.font_size());
    let mut proto = picker.new_resize_protocol(src.clone());
    proto.resize_encode(&Resize::Fit(None), target);
    anim.preview_area = Some(area_key);
    anim.preview_target = target;
    anim.preview_proto = Some(proto);
}
