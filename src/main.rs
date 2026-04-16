//! `glry` — TUI image gallery.

mod app;
mod cache;
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
use ratatui_image::picker::Picker;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use crate::app::App;
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

    install_panic_hook();
    let mut terminal = init_terminal()?;
    let result = run(&mut terminal, start_dir, picker);
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

fn run(terminal: &mut Term, start_dir: PathBuf, picker: Picker) -> Result<()> {
    let cache_dir = cache::cache_dir()?;
    let worker = ThumbWorker::new(cache_dir);
    let mut app = App::new(start_dir, picker, worker)?;

    while !app.should_quit {
        // Drain any completed background loads before rendering.
        app.drain_loads();

        execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
        terminal.draw(|f| ui::render(f, &mut app))?;
        execute!(terminal.backend_mut(), EndSynchronizedUpdate)?;

        // Wait up to 100 ms for an input event; loop again either way so
        // background completions are picked up promptly.
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => app.handle_key(k)?,
                Event::Resize(_, _) => {
                    // Rebuilt grid Protocols are tied to the old cell size; if
                    // the user dramatically resizes, we keep them as-is for v1.
                    // StatefulProtocols (fullscreen / preview) auto-resize.
                }
                _ => {}
            }
        }
    }

    Ok(())
}
