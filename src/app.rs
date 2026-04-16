//! Application state and key dispatch.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use image::DynamicImage;
use ratatui::layout::Rect;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::{Protocol, StatefulProtocol};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ai::{AiConfig, AiWorker};
use crate::config::Theme;
use crate::scan::{self, Entry, ImageEntry};
use crate::thumbnail::{LoadKind, LoadPayload, ThumbWorker};

/// Per-path state of the AI describe feature. Stored in
/// [`App::ai_results`] so pressing `a` on an already-described image just
/// re-opens the overlay without a new round trip.
pub enum AiState {
    /// Overlay opened before the full-res image was decoded. Transitions
    /// to [`AiState::Requesting`] once decoding finishes.
    AwaitingDecode,
    /// The describe request is in flight on the gateway.
    Requesting,
    /// Final assistant content.
    Ready(String),
    /// Request failed.
    Error(String),
}

/// Fixed grid cell size in terminal cells. Each thumbnail occupies this area.
pub const GRID_CELL_W: u16 = 16;
pub const GRID_CELL_H: u16 = 8;
pub const GRID_GAP: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Grid,
    List,
}

/// Cached "fill" mode protocol for the fullscreen viewer. Stored outside
/// `fulls` because it's specific to the current area's aspect ratio and gets
/// rebuilt when that changes.
pub struct FillProto {
    pub path: PathBuf,
    pub area_w: u16,
    pub area_h: u16,
    pub proto: StatefulProtocol,
}

/// A decoded animated image kept in memory for fullscreen playback.
///
/// Holds one fit-mode `StatefulProtocol` per frame (built eagerly on load,
/// so each frame-switch is just a map lookup), the raw `DynamicImage` for
/// each frame (used to build fill-mode protocols on demand), and the
/// per-frame delays. `current`/`last_advance` track playback position.
///
/// `fit_protos` and `fill_protos` cache one pre-encoded protocol per frame
/// for the area in `fit_area`/`fill_area`. Caching matters: each fresh
/// protocol gets a unique terminal image id and re-uploads its pixels, so
/// rebuilding mid-playback would flood the terminal and evict grid
/// thumbnails on Kitty. Both caches are invalidated when the area changes
/// and re-built eagerly so frame advances during playback are pure
/// placeholder swaps.
///
/// `fit_target` and `fill_target` are the centered render rects (computed
/// from the source dims) that the cached protos are encoded for. Rendering
/// at these rects avoids `Resize::Scale`'s upscale-to-area, which would
/// blow up each frame's upload to terminal-pixel size.
pub struct Animation {
    pub frames: Vec<StatefulProtocol>,
    pub images: Vec<DynamicImage>,
    pub delays: Vec<Duration>,
    pub current: usize,
    pub last_advance: Instant,
    pub original_dims: (u32, u32),
    pub fit_area: Option<(u16, u16)>,
    pub fit_target: Rect,
    pub fill_protos: Vec<Option<StatefulProtocol>>,
    pub fill_area: Option<(u16, u16)>,
    pub fill_target: Rect,
    /// Static first-frame protocol for the list-view preview. Kept separate
    /// from the fullscreen caches so switching list ↔ fullscreen doesn't
    /// re-encode (and re-upload) every frame on every mode change.
    pub preview_proto: Option<StatefulProtocol>,
    pub preview_area: Option<(u16, u16)>,
    pub preview_target: Rect,
}

impl Animation {
    /// Advance `current` by as many frames as the elapsed time covers.
    /// Returns `true` if the frame index changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        if self.frames.len() <= 1 {
            return false;
        }
        let mut advanced = false;
        // Cap the catch-up to one full loop of frames so a long sleep
        // (e.g. laptop suspend) doesn't turn this into a tight loop.
        for _ in 0..self.frames.len() {
            let delay = self.delays[self.current];
            if now.duration_since(self.last_advance) < delay {
                return advanced;
            }
            self.current = (self.current + 1) % self.frames.len();
            self.last_advance += delay;
            advanced = true;
        }
        // Still behind after a full cycle: snap forward so the next tick is
        // measured from now rather than the distant past.
        self.last_advance = now;
        advanced
    }

    /// How long until the current frame's delay elapses. `Duration::ZERO`
    /// means a tick is already due.
    pub fn next_tick_in(&self, now: Instant) -> Duration {
        if self.frames.len() <= 1 {
            return Duration::from_secs(3600);
        }
        let delay = self.delays[self.current];
        let elapsed = now.duration_since(self.last_advance);
        delay.saturating_sub(elapsed)
    }
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
    /// Config default for whether bars start hidden on fullscreen entry.
    pub hide_bars_default: bool,
    /// Current state: hide header/status bars while in fullscreen.
    pub fullscreen_bars_hidden: bool,
    /// When true, the fullscreen viewer crops the image to fill the area
    /// instead of letterboxing it. Toggled with `c`.
    pub fullscreen_crop: bool,

    /// Built grid-thumbnail protocols, keyed by source image path.
    pub thumbs: HashMap<PathBuf, Protocol>,
    /// Built resizable protocols (used by both fullscreen and list-preview).
    pub fulls: HashMap<PathBuf, StatefulProtocol>,
    /// Decoded animations keyed by source image path. Populated instead of
    /// `fulls` for animated formats (e.g. GIF).
    pub animations: HashMap<PathBuf, Animation>,
    /// Downscaled `DynamicImage` kept alongside each entry in `fulls`, so the
    /// fullscreen "fill" mode can rebuild an aspect-cropped protocol without
    /// re-decoding from disk.
    pub full_images: HashMap<PathBuf, DynamicImage>,
    /// Single-slot cache for the fullscreen "fill" mode protocol: the image
    /// is center-cropped to the current area's aspect ratio so `Resize::Fit`
    /// renders it edge-to-edge without letterboxing. Rebuilt when the path or
    /// area dimensions change.
    pub fill_proto: Option<FillProto>,
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

    /// Color palette loaded from the user config.
    pub theme: Theme,

    /// AI config + runtime state. `ai_active` is `true` only when the user
    /// enabled the feature AND a token was found in the environment; UI
    /// paths check this single flag instead of re-checking the env each time.
    pub ai_cfg: AiConfig,
    pub ai_token: Option<String>,
    pub ai_active: bool,
    pub ai_worker: AiWorker,
    /// When `Some`, the fullscreen overlay is rendered for this path.
    /// Toggled by the `a` key (and cleared on Esc).
    pub ai_overlay: Option<PathBuf>,
    /// Cached describe states, keyed by image path. A `Loading` entry means
    /// a dispatch is in flight; replaced with `Ready` or `Error` on drain.
    pub ai_results: HashMap<PathBuf, AiState>,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        start_dir: PathBuf,
        picker: Arc<Picker>,
        worker: ThumbWorker,
        theme: Theme,
        hide_bars_default: bool,
        ai_cfg: AiConfig,
        ai_token: Option<String>,
        ai_active: bool,
    ) -> Result<Self> {
        let entries = scan::scan(&start_dir)?;
        let selected = first_selectable(&entries);
        let loading_proto = picker.new_resize_protocol(DynamicImage::new_rgba8(1, 1));
        Ok(Self {
            cwd: start_dir,
            entries,
            selected,
            view: ViewMode::Grid,
            fullscreen_idx: None,
            hide_bars_default,
            fullscreen_bars_hidden: hide_bars_default,
            fullscreen_crop: false,
            thumbs: HashMap::new(),
            fulls: HashMap::new(),
            animations: HashMap::new(),
            full_images: HashMap::new(),
            fill_proto: None,
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
            theme,
            ai_cfg,
            ai_token,
            ai_active,
            ai_worker: AiWorker::new(),
            ai_overlay: None,
            ai_results: HashMap::new(),
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
        self.animations.clear();
        self.full_images.clear();
        self.fill_proto = None;
        self.full_dims.clear();
        self.errors.clear();
        self.requested.clear();
        self.fullscreen_idx = None;
        self.ai_overlay = None;
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
    /// Animated formats (e.g. GIF) are routed to the animation loader so the
    /// viewer can play them back.
    pub fn ensure_full(&mut self, path: &PathBuf) {
        if self.errors.contains_key(path) {
            return;
        }
        if crate::thumbnail::is_animated_path(path) {
            let key = (path.clone(), LoadKind::Animation);
            if self.animations.contains_key(path) || self.requested.contains(&key) {
                return;
            }
            self.requested.insert(key);
            self.worker.dispatch_animation(path.clone());
        } else {
            let key = (path.clone(), LoadKind::Full);
            if self.fulls.contains_key(path) || self.requested.contains(&key) {
                return;
            }
            self.requested.insert(key);
            self.worker.dispatch_full(path.clone());
        }
    }

    /// Returns `true` if any in-flight loads are pending (used for adaptive
    /// poll timing in the event loop).
    pub fn has_pending_loads(&self) -> bool {
        !self.requested.is_empty()
    }

    /// Drain completed loads from the worker and turn them into protocols.
    /// Returns `true` if any loads were processed.
    pub fn drain_loads(&mut self) -> bool {
        let ai_changed = self.drain_ai();
        let completed = self.worker.drain();
        if completed.is_empty() {
            return ai_changed;
        }
        for done in completed {
            self.requested
                .remove(&(done.source_path.clone(), done.kind));
            match done.payload {
                Err(e) => {
                    self.errors.insert(done.source_path, format!("{e:#}"));
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
                    self.full_images
                        .insert(done.source_path.clone(), image.clone());
                    let proto = self.picker.new_resize_protocol(image);
                    self.fulls.insert(done.source_path, proto);
                }
                Ok(LoadPayload::Animation {
                    frames,
                    original_dims,
                }) => {
                    self.full_dims
                        .insert(done.source_path.clone(), original_dims);
                    let mut protos = Vec::with_capacity(frames.len());
                    let mut images = Vec::with_capacity(frames.len());
                    let mut delays = Vec::with_capacity(frames.len());
                    for f in frames {
                        protos.push(self.picker.new_resize_protocol(f.image.clone()));
                        images.push(f.image);
                        delays.push(f.delay);
                    }
                    let frame_count = protos.len();
                    self.animations.insert(
                        done.source_path,
                        Animation {
                            frames: protos,
                            images,
                            delays,
                            current: 0,
                            last_advance: Instant::now(),
                            original_dims,
                            fit_area: None,
                            fit_target: Rect::default(),
                            fill_protos: (0..frame_count).map(|_| None).collect(),
                            fill_area: None,
                            fill_target: Rect::default(),
                            preview_proto: None,
                            preview_area: None,
                            preview_target: Rect::default(),
                        },
                    );
                }
            }
        }
        true
    }

    /// Advance any animation whose current frame's delay has elapsed.
    /// Returns `true` if any frame changed (caller should redraw).
    pub fn tick_animations(&mut self) -> bool {
        let Some(idx) = self.fullscreen_idx else {
            return false;
        };
        let Some(Entry::Image(img)) = self.entries.get(idx) else {
            return false;
        };
        let Some(anim) = self.animations.get_mut(&img.path) else {
            return false;
        };
        anim.tick(Instant::now())
    }

    /// The shortest wait until the next animation frame should be shown for
    /// the currently-visible animation, if any.
    pub fn next_animation_tick(&self) -> Option<Duration> {
        let idx = self.fullscreen_idx?;
        let entry = self.entries.get(idx)?;
        let Entry::Image(img) = entry else {
            return None;
        };
        let anim = self.animations.get(&img.path)?;
        Some(anim.next_tick_in(Instant::now()))
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
        // Clear any transient status from the previous keypress so messages
        // like "Fit"/"Fill"/"Copied …" don't shadow the per-image info bar.
        self.status = None;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                if self.ai_overlay.is_some() {
                    self.ai_overlay = None;
                } else {
                    self.fullscreen_idx = None;
                }
            }
            KeyCode::Left | KeyCode::Char('h') => self.fullscreen_step(-1),
            KeyCode::Right | KeyCode::Char('l') => self.fullscreen_step(1),
            KeyCode::Char('y') => self.copy_to_clipboard(),
            KeyCode::Char('b') => self.fullscreen_bars_hidden = !self.fullscreen_bars_hidden,
            KeyCode::Char('c') => {
                self.fullscreen_crop = !self.fullscreen_crop;
                self.status = Some(if self.fullscreen_crop { "Fill" } else { "Fit" }.to_string());
            }
            KeyCode::Char('a') => self.toggle_ai_overlay(),
            _ => {}
        }
    }

    /// Toggle the AI describe overlay for the current fullscreen image.
    /// Dispatches a describe request on first open when no prior result is
    /// cached. Requires the AI feature to be active; otherwise shows a
    /// status hint.
    fn toggle_ai_overlay(&mut self) {
        if self.ai_overlay.is_some() {
            self.ai_overlay = None;
            return;
        }
        if !self.ai_active {
            self.status = Some(
                "AI disabled — set ai_enabled = true in config and export SWIFTROUTER_API_KEY"
                    .to_string(),
            );
            return;
        }
        let Some(idx) = self.fullscreen_idx else {
            return;
        };
        let Some(Entry::Image(img)) = self.entries.get(idx).cloned() else {
            return;
        };
        self.ai_overlay = Some(img.path.clone());
        if self.ai_results.contains_key(&img.path) {
            // A prior Ready/Error result, or a dispatch already in flight.
            return;
        }
        // Can't describe what we haven't decoded yet. Kick off the full
        // load if needed and mark the overlay as awaiting decode; the next
        // drain will notice the image and dispatch the describe.
        if let Some(image) = self.full_images.get(&img.path).cloned() {
            self.dispatch_describe(img.path, image);
        } else {
            self.ai_results
                .insert(img.path.clone(), AiState::AwaitingDecode);
            self.ensure_full(&img.path);
        }
    }

    fn dispatch_describe(&mut self, path: PathBuf, image: DynamicImage) {
        let Some(token) = self.ai_token.clone() else {
            self.ai_results.insert(
                path,
                AiState::Error("SWIFTROUTER_API_KEY not set".to_string()),
            );
            return;
        };
        self.ai_results.insert(path.clone(), AiState::Requesting);
        self.ai_worker
            .dispatch(path, image, self.ai_cfg.clone(), token);
    }

    /// Drain completed describe jobs and, if an overlay was waiting on a
    /// decode, dispatch now that the image is ready. Returns `true` if any
    /// AI state changed (caller should redraw).
    fn drain_ai(&mut self) -> bool {
        let mut changed = false;
        for done in self.ai_worker.drain() {
            match done.result {
                Ok(s) => {
                    self.ai_results.insert(done.path, AiState::Ready(s));
                }
                Err(e) => {
                    self.ai_results
                        .insert(done.path, AiState::Error(format!("{e:#}")));
                }
            }
            changed = true;
        }
        // Was an overlay blocked on decode? Dispatch now that the image exists.
        let to_dispatch = self
            .ai_overlay
            .clone()
            .filter(|p| matches!(self.ai_results.get(p), Some(AiState::AwaitingDecode)))
            .filter(|p| self.full_images.contains_key(p));
        if let Some(path) = to_dispatch
            && let Some(image) = self.full_images.get(&path).cloned()
        {
            self.dispatch_describe(path, image);
            changed = true;
        }
        changed
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
                // Overlay always describes the currently-visible image;
                // dismiss it on navigation so the user doesn't see a stale
                // description sitting over a different image.
                self.ai_overlay = None;
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
                self.fullscreen_bars_hidden = self.hide_bars_default;
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

    let status = child.wait().with_context(|| format!("waiting for {cmd}"))?;
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
    if let Some(i) = entries
        .iter()
        .position(|e| matches!(e, Entry::SubDir { .. }))
    {
        return i;
    }
    0
}
