//! Application state and key dispatch.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use image::DynamicImage;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::{Protocol, StatefulProtocol};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::scan::{self, Entry, ImageEntry};
use crate::thumbnail::{LoadKind, LoadPayload, ThumbWorker};

/// Fixed grid cell size in terminal cells. Each thumbnail occupies this area.
pub const GRID_CELL_W: u16 = 16;
pub const GRID_CELL_H: u16 = 8;
pub const GRID_GAP: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Grid,
    List,
}

pub struct App {
    /// Current working directory.
    pub cwd: PathBuf,
    /// Entries in the current directory: parent + subdirs + images.
    pub entries: Vec<Entry>,
    /// Currently selected entry index (0..entries.len()).
    pub selected: usize,
    /// Active view mode (toggled with Tab).
    pub view: ViewMode,
    /// When Some, render the fullscreen viewer for this entry index.
    pub fullscreen_idx: Option<usize>,

    /// Built grid-thumbnail protocols, keyed by source image path.
    pub thumbs: HashMap<PathBuf, Protocol>,
    /// Built resizable protocols (used by both fullscreen and list-preview).
    pub fulls: HashMap<PathBuf, StatefulProtocol>,
    /// Original (pre-display) pixel dimensions of fully-loaded images.
    pub full_dims: HashMap<PathBuf, (u32, u32)>,
    /// Per-path error message, if a load failed.
    pub errors: HashMap<PathBuf, String>,
    /// In-flight (path, kind) to dedupe dispatches.
    pub requested: HashSet<(PathBuf, LoadKind)>,

    pub picker: Arc<Picker>,
    pub worker: ThumbWorker,
    /// Tiny placeholder protocol rendered while a full image is loading.
    /// Going through StatefulImage ensures protocol-specific cleanup of the
    /// previous image (e.g. Kitty overlay deletion).
    pub loading_proto: StatefulProtocol,

    /// Vim "gg" — true after first 'g' is pressed; cleared on next key.
    pub pending_g: bool,
    pub should_quit: bool,
    /// Status-bar message (transient feedback).
    pub status: Option<String>,
    /// Whether the UI needs to be redrawn.
    pub dirty: bool,

    /// Number of columns the grid view rendered last frame. Used so j/k
    /// jumps a row's worth of entries.
    pub last_grid_cols: u16,
    /// Top row offset for the grid view's vertical scroll.
    pub grid_scroll_row: usize,
}

impl App {
    pub fn new(start_dir: PathBuf, picker: Arc<Picker>, worker: ThumbWorker) -> Result<Self> {
        let entries = scan::scan(&start_dir)?;
        let selected = first_selectable(&entries);
        let loading_proto = picker.new_resize_protocol(DynamicImage::new_rgba8(1, 1));
        Ok(Self {
            cwd: start_dir,
            entries,
            selected,
            view: ViewMode::Grid,
            fullscreen_idx: None,
            thumbs: HashMap::new(),
            fulls: HashMap::new(),
            full_dims: HashMap::new(),
            errors: HashMap::new(),
            requested: HashSet::new(),
            picker,
            worker,
            loading_proto,
            pending_g: false,
            should_quit: false,
            status: None,
            dirty: true,
            last_grid_cols: 1,
            grid_scroll_row: 0,
        })
    }

    /// Re-scan a directory. Clears per-directory caches (in-memory only;
    /// the on-disk thumbnail cache is preserved across navigation).
    pub fn enter_dir(&mut self, path: PathBuf) -> Result<()> {
        let entries = scan::scan(&path)?;
        self.cwd = path;
        self.selected = first_selectable(&entries);
        self.entries = entries;
        self.thumbs.clear();
        self.fulls.clear();
        self.full_dims.clear();
        self.errors.clear();
        self.requested.clear();
        self.fullscreen_idx = None;
        self.status = None;
        self.grid_scroll_row = 0;
        Ok(())
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    /// Request a grid thumbnail load if it isn't already in progress or done.
    pub fn ensure_thumb(&mut self, entry: &ImageEntry) {
        let key = (entry.path.clone(), LoadKind::Thumb);
        if self.thumbs.contains_key(&entry.path)
            || self.errors.contains_key(&entry.path)
            || self.requested.contains(&key)
        {
            return;
        }
        self.requested.insert(key);
        self.worker.dispatch_thumb(entry.clone());
    }

    /// Request a full-resolution load if it isn't already in progress or done.
    pub fn ensure_full(&mut self, path: &PathBuf) {
        let key = (path.clone(), LoadKind::Full);
        if self.fulls.contains_key(path) || self.requested.contains(&key) {
            return;
        }
        self.requested.insert(key);
        self.worker.dispatch_full(path.clone());
    }

    /// Returns `true` if any in-flight loads are pending (used for adaptive
    /// poll timing in the event loop).
    pub fn has_pending_loads(&self) -> bool {
        !self.requested.is_empty()
    }

    /// Drain completed loads from the worker and turn them into protocols.
    /// Returns `true` if any loads were processed.
    pub fn drain_loads(&mut self) -> bool {
        let completed = self.worker.drain();
        if completed.is_empty() {
            return false;
        }
        for done in completed {
            self.requested.remove(&(done.source_path.clone(), done.kind));
            match done.payload {
                Err(e) => {
                    self.errors
                        .insert(done.source_path, format!("{e:#}"));
                }
                Ok(LoadPayload::Thumb(proto)) => {
                    self.thumbs.insert(done.source_path, proto);
                }
                Ok(LoadPayload::Full {
                    image,
                    original_dims,
                }) => {
                    self.full_dims
                        .insert(done.source_path.clone(), original_dims);
                    let proto = self.picker.new_resize_protocol(image);
                    self.fulls.insert(done.source_path, proto);
                }
            }
        }
        true
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Fullscreen viewer has its own key map.
        if self.fullscreen_idx.is_some() {
            self.handle_key_fullscreen(key);
            return Ok(());
        }

        let was_pending_g = self.pending_g;
        self.pending_g = false;
        self.status = None;

        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            (KeyCode::Tab, _) => {
                self.view = match self.view {
                    ViewMode::Grid => ViewMode::List,
                    ViewMode::List => ViewMode::Grid,
                };
            }
            (KeyCode::Char('g'), _) => {
                if was_pending_g {
                    self.select_first();
                } else {
                    self.pending_g = true;
                }
            }
            (KeyCode::Char('G'), _) => self.select_last(),
            (KeyCode::Char('h'), _) | (KeyCode::Left, _) => self.move_horiz(-1),
            (KeyCode::Char('l'), _) | (KeyCode::Right, _) => self.move_horiz(1),
            (KeyCode::Char('j'), _) | (KeyCode::Down, _) => self.move_vert(1),
            (KeyCode::Char('k'), _) | (KeyCode::Up, _) => self.move_vert(-1),
            (KeyCode::PageDown, _) => self.page(1),
            (KeyCode::PageUp, _) => self.page(-1),
            (KeyCode::Home, _) => self.select_first(),
            (KeyCode::End, _) => self.select_last(),
            (KeyCode::Enter, _) => self.activate_selection()?,
            (KeyCode::Backspace, _) => self.go_up()?,
            (KeyCode::Char('y'), _) => self.copy_to_clipboard(),
            _ => {}
        }
        Ok(())
    }

    fn handle_key_fullscreen(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.fullscreen_idx = None,
            KeyCode::Left | KeyCode::Char('h') => self.fullscreen_step(-1),
            KeyCode::Right | KeyCode::Char('l') => self.fullscreen_step(1),
            KeyCode::Char('y') => self.copy_to_clipboard(),
            _ => {}
        }
    }

    fn fullscreen_step(&mut self, dir: isize) {
        let Some(start) = self.fullscreen_idx else {
            return;
        };
        let n = self.entries.len() as isize;
        if n == 0 {
            return;
        }
        let mut i = start as isize;
        for _ in 0..n {
            i = (i + dir).rem_euclid(n);
            if let Some(Entry::Image(img)) = self.entries.get(i as usize).cloned() {
                self.fullscreen_idx = Some(i as usize);
                self.selected = i as usize;
                // Start loading immediately rather than waiting for the next render.
                self.ensure_full(&img.path);
                // Preload the next image in the same direction for snappier navigation.
                self.preload_adjacent(i as usize, dir);
                return;
            }
        }
    }

    /// Preload the next image in `dir` from `current` so it's ready if the
    /// user keeps navigating in the same direction.
    fn preload_adjacent(&mut self, current: usize, dir: isize) {
        let n = self.entries.len() as isize;
        let mut i = current as isize;
        for _ in 0..n {
            i = (i + dir).rem_euclid(n);
            if let Some(Entry::Image(img)) = self.entries.get(i as usize).cloned() {
                self.ensure_full(&img.path);
                return;
            }
        }
    }

    fn move_vert(&mut self, delta: isize) {
        // Grid: jump a row's worth of entries. List: single step.
        let step = match self.view {
            ViewMode::Grid => self.last_grid_cols.max(1) as isize,
            ViewMode::List => 1,
        };
        self.move_linear(delta * step);
    }

    fn move_horiz(&mut self, delta: isize) {
        // List mode has no horizontal navigation.
        if self.view == ViewMode::Grid {
            self.move_linear(delta);
        }
    }

    fn page(&mut self, dir: isize) {
        match self.view {
            ViewMode::Grid => self.move_vert(3 * dir),
            ViewMode::List => self.move_linear(10 * dir),
        }
    }

    /// Linear move of the selection by `delta`, clamped to entry range.
    pub fn move_linear(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let n = self.entries.len() as isize;
        let new = (self.selected as isize + delta).clamp(0, n - 1);
        self.selected = new as usize;
    }

    pub fn select_first(&mut self) {
        if !self.entries.is_empty() {
            self.selected = 0;
        }
    }

    pub fn select_last(&mut self) {
        if !self.entries.is_empty() {
            self.selected = self.entries.len() - 1;
        }
    }

    fn activate_selection(&mut self) -> Result<()> {
        let Some(entry) = self.selected_entry().cloned() else {
            return Ok(());
        };
        match entry {
            Entry::Parent(p) | Entry::SubDir { path: p, .. } => self.enter_dir(p)?,
            Entry::Image(img) => {
                self.fullscreen_idx = Some(self.selected);
                self.ensure_full(&img.path);
            }
        }
        Ok(())
    }

    fn go_up(&mut self) -> Result<()> {
        if let Some(parent) = self.cwd.parent().map(|p| p.to_path_buf()) {
            self.enter_dir(parent)?;
        }
        Ok(())
    }

    /// Copy the currently selected image to the system clipboard.
    fn copy_to_clipboard(&mut self) {
        let path = match self.selected_entry() {
            Some(Entry::Image(img)) => img.path.clone(),
            _ => return,
        };

        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();

        match crate::thumbnail::load_original(&path) {
            Ok(img) => match copy_image(img) {
                Ok(()) => self.status = Some(format!("Copied {name}")),
                Err(e) => self.status = Some(format!("Clipboard error: {e:#}")),
            },
            Err(e) => self.status = Some(format!("Could not load image: {e:#}")),
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn copy_image(img: DynamicImage) -> Result<()> {
    let rgba = img.to_rgba8();
    let data = arboard::ImageData {
        width: rgba.width() as usize,
        height: rgba.height() as usize,
        bytes: std::borrow::Cow::Borrowed(rgba.as_raw()),
    };
    arboard::Clipboard::new()
        .and_then(|mut cb| cb.set_image(data))
        .map_err(|e| anyhow::anyhow!("{e}"))
}

// On Linux the clipboard is owned by the running process, so `arboard`'s
// content disappears when glry exits. `wl-copy` and `xclip` both fork a
// daemon that outlives us, so shell out to whichever matches the session.
#[cfg(target_os = "linux")]
fn copy_image(img: DynamicImage) -> Result<()> {
    use anyhow::Context;
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .context("encoding PNG for clipboard")?;

    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let (cmd, args, install_hint): (&str, &[&str], &str) = if wayland {
        (
            "wl-copy",
            &["--type", "image/png"],
            "install `wl-clipboard` (e.g. `pacman -S wl-clipboard`)",
        )
    } else {
        (
            "xclip",
            &["-selection", "clipboard", "-t", "image/png", "-i"],
            "install `xclip` (e.g. `pacman -S xclip`)",
        )
    };

    let mut child = match Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("`{cmd}` not found — {install_hint}");
        }
        Err(e) => return Err(e).context(format!("spawning {cmd}")),
    };

    let mut stdin = child.stdin.take().expect("stdin is piped");
    stdin
        .write_all(&png)
        .with_context(|| format!("writing image to {cmd}"))?;
    drop(stdin);

    let status = child
        .wait()
        .with_context(|| format!("waiting for {cmd}"))?;
    if !status.success() {
        anyhow::bail!("{cmd} exited with {status}");
    }
    Ok(())
}

/// Returns the index of the first entry that is selectable on entry into a
/// directory: prefer the first image, then the first subdir, else 0.
fn first_selectable(entries: &[Entry]) -> usize {
    if let Some(i) = entries.iter().position(|e| matches!(e, Entry::Image(_))) {
        return i;
    }
    if let Some(i) = entries.iter().position(|e| matches!(e, Entry::SubDir { .. })) {
        return i;
    }
    0
}
