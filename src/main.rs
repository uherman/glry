//! `glry` — TUI image gallery.

mod app;
mod cache;
mod config;
mod scan;
mod thumbnail;
mod ui;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
    disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui_image::picker::Picker;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::app::{App, GRID_CELL_H, GRID_CELL_W};
use crate::config::Config;
use crate::thumbnail::ThumbWorker;

#[derive(Parser, Debug)]
#[command(name = "glry", version, about = "TUI image gallery")]
struct Cli {
    /// Directory to open (defaults to the current directory).
    #[arg(default_value = ".")]
    path: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let start_dir = cli
        .path
        .canonicalize()
        .with_context(|| format!("opening {}", cli.path.display()))?;
    if !start_dir.is_dir() {
        anyhow::bail!("{} is not a directory", start_dir.display());
    }

    // Query the terminal for graphics capabilities BEFORE entering the
    // alternate screen / raw mode — the query writes CSI sequences to stdout
    // and reads replies from stdin, and is cleaner outside the TUI display.
    let picker = Picker::from_query_stdio()
        .context("querying terminal for graphics capabilities")?;

    // Load user config before entering the TUI so parse errors are visible.
    let cfg = config::load();

    install_panic_hook();
    let mut terminal = init_terminal()?;
    let result = run(&mut terminal, start_dir, picker, cfg);
    restore_terminal(&mut terminal).ok();
    result
}

type Term = Terminal<CrosstermBackend<Stdout>>;

fn init_terminal() -> Result<Term> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend).context("create terminal")?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Term) -> Result<()> {
    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    Ok(())
}

/// Restore the terminal on panic so the user's shell isn't left in raw mode.
fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original(info);
    }));
}

fn run(terminal: &mut Term, start_dir: PathBuf, picker: Picker, cfg: Config) -> Result<()> {
    let cache_dir = cache::cache_dir()?;

    // Compute the maximum useful pixel dimension for full-resolution images.
    // Anything larger than the terminal's pixel area is wasted work.
    let term_size = terminal.size()?;
    let font_size = picker.font_size();
    let max_full_dim = (term_size.width as u32 * font_size.0 as u32)
        .max(term_size.height as u32 * font_size.1 as u32)
        .max(1920); // floor at 1080p so small terminals still get decent quality

    // If cropping is on, center-crop to a square so thumbnails always render
    // with equal width and height regardless of the terminal's font aspect.
    let crop = cfg.thumbnail_crop.then_some((1, 1));

    let picker = Arc::new(picker);
    let thumb_area = Rect::new(0, 0, GRID_CELL_W, GRID_CELL_H);
    let worker = ThumbWorker::new(cache_dir, Arc::clone(&picker), thumb_area, max_full_dim, crop);
    let mut app = App::new(start_dir, picker, worker, cfg.theme, cfg.fullscreen_hide_bars)?;

    while !app.should_quit {
        // Drain any completed background loads before rendering.
        if app.drain_loads() {
            app.dirty = true;
        }
        // Advance the fullscreen animation (if any) before rendering.
        if app.tick_animations() {
            app.dirty = true;
        }

        if app.dirty {
            execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
            terminal.draw(|f| ui::render(f, &mut app))?;
            execute!(terminal.backend_mut(), EndSynchronizedUpdate)?;
            app.dirty = false;
        }

        // Adaptive poll: short timeout when loads are in-flight for responsive
        // updates, longer when idle to save CPU. If an animation is playing,
        // wake in time for its next frame.
        let base = if app.has_pending_loads() {
            Duration::from_millis(32)
        } else {
            Duration::from_millis(100)
        };
        let timeout = match app.next_animation_tick() {
            Some(d) => base.min(d.max(Duration::from_millis(1))),
            None => base,
        };

        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    app.handle_key(k)?;
                    app.dirty = true;
                }
                Event::Resize(_, _) => {
                    app.dirty = true;
                }
                _ => {}
            }
        }
    }

    Ok(())
}
