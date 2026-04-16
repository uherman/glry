//! Grid view: scrollable grid of fixed-size thumbnail cells.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Paragraph;
use ratatui_image::Image;

use crate::app::{App, GRID_CELL_H, GRID_CELL_W, GRID_GAP};
use crate::scan::Entry;

/// Each cell occupies the thumbnail area plus a 1-row caption strip.
const CELL_TOTAL_W: u16 = GRID_CELL_W + GRID_GAP;
const CAPTION_H: u16 = 1;
const CELL_TOTAL_H: u16 = GRID_CELL_H + CAPTION_H + GRID_GAP;

pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
    if area.width < GRID_CELL_W || area.height < GRID_CELL_H {
        return; // too small to draw anything sensible
    }

    let cols = ((area.width + GRID_GAP) / CELL_TOTAL_W).max(1);
    let rows_visible = ((area.height + GRID_GAP) / CELL_TOTAL_H).max(1) as usize;
    app.last_grid_cols = cols;

    // Auto-scroll so the selected entry stays visible.
    let sel_row = app.selected / cols as usize;
    if sel_row < app.grid_scroll_row {
        app.grid_scroll_row = sel_row;
    } else if sel_row >= app.grid_scroll_row + rows_visible {
        app.grid_scroll_row = sel_row + 1 - rows_visible;
    }

    let total_rows = app.entries.len().div_ceil(cols as usize);
    let max_scroll = total_rows.saturating_sub(rows_visible);
    if app.grid_scroll_row > max_scroll {
        app.grid_scroll_row = max_scroll;
    }

    // First, kick off thumbnail loads for every image entry that is visible.
    let first_visible = app.grid_scroll_row * cols as usize;
    let last_visible = ((app.grid_scroll_row + rows_visible) * cols as usize)
        .min(app.entries.len());
    for idx in first_visible..last_visible {
        if let Some(Entry::Image(img)) = app.entries.get(idx).cloned().as_ref() {
            app.ensure_thumb(img);
        }
    }

    // Now render the visible cells.
    for row in 0..rows_visible {
        let entry_row = app.grid_scroll_row + row;
        for col in 0..cols as usize {
            let idx = entry_row * cols as usize + col;
            if idx >= app.entries.len() {
                break;
            }
            let cell_x = area.x + (col as u16) * CELL_TOTAL_W;
            let cell_y = area.y + (row as u16) * CELL_TOTAL_H;
            if cell_y + GRID_CELL_H + CAPTION_H > area.y + area.height {
                break;
            }
            let img_area = Rect::new(cell_x, cell_y, GRID_CELL_W, GRID_CELL_H);
            let cap_area = Rect::new(cell_x, cell_y + GRID_CELL_H, GRID_CELL_W, CAPTION_H);
            let entry = &app.entries[idx];
            let selected = idx == app.selected;
            render_cell(f, img_area, cap_area, entry, app, selected);
        }
    }
}

fn render_cell(
    f: &mut Frame,
    img_area: Rect,
    cap_area: Rect,
    entry: &Entry,
    app: &App,
    selected: bool,
) {
    // The image area: thumbnail for images, glyph placeholder for dirs/parent.
    match entry {
        Entry::Parent(_) => {
            let p = Paragraph::new("\n  [ .. ]")
                .style(Style::default().fg(app.theme.directory_fg));
            f.render_widget(p, img_area);
        }
        Entry::SubDir { .. } => {
            let p = Paragraph::new("\n   📁")
                .style(Style::default().fg(app.theme.directory_fg));
            f.render_widget(p, img_area);
        }
        Entry::Image(img) => {
            if let Some(proto) = app.thumbs.get(&img.path) {
                f.render_widget(Image::new(proto), img_area);
            } else if let Some(err) = app.errors.get(&img.path) {
                let p = Paragraph::new(format!("err\n{err}"))
                    .style(Style::default().fg(app.theme.error_fg));
                f.render_widget(p, img_area);
            } else {
                let p = Paragraph::new("...")
                    .style(Style::default().fg(app.theme.loading_fg));
                f.render_widget(p, img_area);
            }
        }
    }

    // The caption strip: filename, highlighted if selected.
    let name = entry.display_name();
    let truncated = truncate_middle(name, cap_area.width as usize);
    let mut style = Style::default();
    if selected {
        style = style
            .bg(app.theme.selection_bg)
            .fg(app.theme.selection_fg)
            .add_modifier(Modifier::BOLD);
    }
    let p = Paragraph::new(truncated).style(style);
    f.render_widget(p, cap_area);
}

/// Shorten `s` so it fits in `max` columns, preserving the start and end.
fn truncate_middle(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    if max <= 3 {
        return chars.iter().take(max).collect();
    }
    let keep = max - 1; // 1 for the ellipsis '…'
    let head = keep.div_ceil(2);
    let tail = keep / 2;
    let mut out: String = chars.iter().take(head).collect();
    out.push('…');
    out.extend(chars.iter().skip(chars.len() - tail));
    out
}
