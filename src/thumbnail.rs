//! Thumbnail (and full-resolution) image loading.
//!
//! Two entry points used by the background worker:
//! - [`load_thumbnail`]: cached, resized to fit max_dim. Used by the grid view.
//!   The worker also builds the terminal `Protocol` off the main thread.
//! - [`load_full`]: uncached, full-resolution (capped to terminal pixel size).
//!   Used by the fullscreen viewer.
//!
//! Both apply EXIF orientation; the `image` crate does not auto-rotate.

use anyhow::{Context, Result};
use image::imageops::FilterType;
use image::DynamicImage;
use ratatui::layout::Rect;
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use crate::cache;
use crate::scan::ImageEntry;

/// Maximum dimension (width or height in pixels) for grid thumbnails.
pub const THUMB_MAX_DIM: u32 = 256;

/// Load a thumbnail for `entry`, using the on-disk cache if available.
/// Returns the decoded `DynamicImage` (post-rotation, post-resize).
pub fn load_thumbnail(entry: &ImageEntry, cache_dir: &Path) -> Result<DynamicImage> {
    let key = cache::key_for(entry);
    let cached = cache::path_for(cache_dir, key);

    if cached.exists() {
        if let Ok(img) = cache::read_thumbnail(&cached) {
            return Ok(img);
        }
        // Corrupted / old-format cache entry: regenerate.
        let _ = fs::remove_file(&cached);
    }

    let img = decode_with_orientation(&entry.path)?;
    let thumb = img.thumbnail(THUMB_MAX_DIM, THUMB_MAX_DIM);

    if let Err(e) = cache::write_thumbnail(&cached, &thumb) {
        eprintln!("glry: cache write failed: {e}");
    }

    Ok(thumb)
}

/// Load a full-resolution image, applying EXIF rotation, then downscale
/// to at most `max_dim` pixels on the longest side so the main thread
/// spends less time on protocol encoding.
///
/// Returns `(possibly_downscaled_image, original_pixel_dimensions)`.
pub fn load_full(path: &Path, max_dim: u32) -> Result<(DynamicImage, (u32, u32))> {
    let img = decode_with_orientation(path)?;
    let original_dims = (img.width(), img.height());

    let img = if img.width() > max_dim || img.height() > max_dim {
        img.resize(max_dim, max_dim, FilterType::Triangle)
    } else {
        img
    };

    Ok((img, original_dims))
}

/// Decode an image and apply EXIF orientation.
fn decode_with_orientation(path: &Path) -> Result<DynamicImage> {
    // Read EXIF first (cheap — just the file header) so the OS page cache is
    // warm for the full decode that follows.
    let orientation = read_exif_orientation(path).unwrap_or(1);
    let img = image::open(path).with_context(|| format!("decoding {}", path.display()))?;
    Ok(apply_orientation(img, orientation))
}

/// Read the EXIF orientation tag (1-8). Returns None if absent or unreadable.
fn read_exif_orientation(path: &Path) -> Option<u32> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;
    let field = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?;
    field.value.get_uint(0)
}

/// What kind of load was requested. Used for dedup in the `requested` set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoadKind {
    Thumb,
    Full,
}

/// The payload sent back to the main thread once a load completes.
pub enum LoadPayload {
    /// Pre-built grid thumbnail Protocol (ready to insert, no main-thread encoding).
    Thumb(Protocol),
    /// Downscaled image for fullscreen, with the original pixel dimensions.
    Full {
        image: DynamicImage,
        original_dims: (u32, u32),
    },
}

/// A completed image-load result handed back to the main thread.
pub struct LoadDone {
    pub source_path: PathBuf,
    pub kind: LoadKind,
    pub payload: Result<LoadPayload>,
}

/// Background loader: dispatches decode work onto rayon's global pool and
/// posts results onto an MPSC channel for the main thread to drain.
///
/// Thumbnail `Protocol` objects are built on the worker thread so the main
/// thread never blocks on image encoding.
pub struct ThumbWorker {
    tx: Sender<LoadDone>,
    rx: Receiver<LoadDone>,
    cache_dir: PathBuf,
    picker: Arc<Picker>,
    thumb_area: Rect,
    max_full_dim: u32,
}

impl ThumbWorker {
    pub fn new(cache_dir: PathBuf, picker: Arc<Picker>, thumb_area: Rect, max_full_dim: u32) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            cache_dir,
            picker,
            thumb_area,
            max_full_dim,
        }
    }

    /// Dispatch a thumbnail load. The Protocol is built on the worker thread;
    /// the main thread just inserts the finished result.
    pub fn dispatch_thumb(&self, entry: ImageEntry) {
        let tx = self.tx.clone();
        let cache_dir = self.cache_dir.clone();
        let path = entry.path.clone();
        let picker = Arc::clone(&self.picker);
        let area = self.thumb_area;
        rayon::spawn(move || {
            let result = load_thumbnail(&entry, &cache_dir).and_then(|img| {
                picker
                    .new_protocol(img, area, Resize::Fit(None))
                    .map_err(|e| anyhow::anyhow!("{e}"))
            });
            let _ = tx.send(LoadDone {
                source_path: path,
                kind: LoadKind::Thumb,
                payload: result.map(LoadPayload::Thumb),
            });
        });
    }

    /// Dispatch a full-resolution load. The image is downscaled to terminal
    /// pixel dimensions on the worker thread.
    pub fn dispatch_full(&self, path: PathBuf) {
        let tx = self.tx.clone();
        let p = path.clone();
        let max_dim = self.max_full_dim;
        rayon::spawn(move || {
            let result = load_full(&p, max_dim);
            let _ = tx.send(LoadDone {
                source_path: p,
                kind: LoadKind::Full,
                payload: result.map(|(image, original_dims)| LoadPayload::Full {
                    image,
                    original_dims,
                }),
            });
        });
    }

    /// Drain all currently-completed loads (non-blocking).
    pub fn drain(&self) -> Vec<LoadDone> {
        self.rx.try_iter().collect()
    }
}

/// Apply one of the 8 EXIF orientations to an image.
/// 1=identity, 2=flip-h, 3=rot180, 4=flip-v,
/// 5=transpose, 6=rot90-cw, 7=transverse, 8=rot90-ccw.
fn apply_orientation(img: DynamicImage, orientation: u32) -> DynamicImage {
    use image::imageops::{flip_horizontal, flip_vertical, rotate90, rotate180, rotate270};
    match orientation {
        2 => DynamicImage::ImageRgba8(flip_horizontal(&img)),
        3 => DynamicImage::ImageRgba8(rotate180(&img)),
        4 => DynamicImage::ImageRgba8(flip_vertical(&img)),
        5 => {
            let r = rotate90(&img);
            DynamicImage::ImageRgba8(flip_horizontal(&r))
        }
        6 => DynamicImage::ImageRgba8(rotate90(&img)),
        7 => {
            let r = rotate270(&img);
            DynamicImage::ImageRgba8(flip_horizontal(&r))
        }
        8 => DynamicImage::ImageRgba8(rotate270(&img)),
        _ => img,
    }
}
