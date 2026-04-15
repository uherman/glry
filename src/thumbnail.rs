//! Thumbnail (and full-resolution) image loading.
//!
//! Two entry points:
//! - [`load_thumbnail`]: cached, resized to fit max_dim. Used by the grid view.
//! - [`load_full`]: uncached, full-resolution. Used by the fullscreen viewer
//!   (a `StatefulProtocol` then handles fitting to the available area).
//!
//! Both apply EXIF orientation; the `image` crate does not auto-rotate.

use anyhow::{Context, Result};
use image::{DynamicImage, ImageFormat};
use std::fs;
use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

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
        // Cache hit: just decode the cached PNG.
        if let Ok(img) = image::open(&cached) {
            return Ok(img);
        }
        // Corrupted cache entry: fall through and regenerate.
        let _ = fs::remove_file(&cached);
    }

    let img = decode_with_orientation(&entry.path)?;
    let thumb = img.thumbnail(THUMB_MAX_DIM, THUMB_MAX_DIM);

    // Encode to PNG in memory, then atomically write to the cache.
    let mut buf: Vec<u8> = Vec::new();
    thumb
        .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .with_context(|| format!("encoding thumbnail PNG for {}", entry.path.display()))?;
    if let Err(e) = cache::atomic_write(&cached, &buf) {
        // Cache write failure is non-fatal: log to stderr and continue.
        eprintln!("glry: cache write failed: {e}");
    }

    Ok(thumb)
}

/// Load a full-resolution image (uncached), applying EXIF rotation.
pub fn load_full(path: &Path) -> Result<DynamicImage> {
    decode_with_orientation(path)
}

/// Decode an image and apply EXIF orientation.
fn decode_with_orientation(path: &Path) -> Result<DynamicImage> {
    let img = image::open(path).with_context(|| format!("decoding {}", path.display()))?;
    let orientation = read_exif_orientation(path).unwrap_or(1);
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

/// What kind of load was requested. The completion callback uses this so the
/// app knows which slot the result fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoadKind {
    /// Cached, resized thumbnail for the grid view.
    Thumb,
    /// Uncached, full-resolution image for the fullscreen view.
    Full,
}

/// A completed image-load result handed back to the main thread.
pub struct LoadDone {
    pub source_path: PathBuf,
    pub kind: LoadKind,
    pub result: Result<DynamicImage>,
}

/// Background loader: dispatches decode work onto rayon's global pool and
/// posts results onto an MPSC channel for the main thread to drain.
pub struct ThumbWorker {
    tx: Sender<LoadDone>,
    rx: Receiver<LoadDone>,
    cache_dir: PathBuf,
}

impl ThumbWorker {
    pub fn new(cache_dir: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx, cache_dir }
    }

    /// Dispatch a thumbnail load. The result will appear via `try_recv` later.
    pub fn dispatch_thumb(&self, entry: ImageEntry) {
        let tx = self.tx.clone();
        let cache_dir = self.cache_dir.clone();
        let path = entry.path.clone();
        rayon::spawn(move || {
            let result = load_thumbnail(&entry, &cache_dir);
            let _ = tx.send(LoadDone {
                source_path: path,
                kind: LoadKind::Thumb,
                result,
            });
        });
    }

    /// Dispatch a full-resolution load (no cache).
    pub fn dispatch_full(&self, path: PathBuf) {
        let tx = self.tx.clone();
        let p = path.clone();
        rayon::spawn(move || {
            let result = load_full(&p);
            let _ = tx.send(LoadDone {
                source_path: p,
                kind: LoadKind::Full,
                result,
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

