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
use image::imageops::{self, FilterType};
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

/// Target pixel aspect ratio (width, height) for center-cropped thumbnails.
/// When set, [`load_thumbnail`] crops the decoded image to this aspect before
/// resizing so every rendered cell is filled uniformly. `None` preserves the
/// original aspect (letterbox).
pub type CropAspect = Option<(u32, u32)>;

/// Load a thumbnail for `entry`, using the on-disk cache if available.
/// Returns the decoded `DynamicImage` (post-rotation, post-optional-crop,
/// post-resize).
pub fn load_thumbnail(
    entry: &ImageEntry,
    cache_dir: &Path,
    crop: CropAspect,
) -> Result<DynamicImage> {
    let key = cache::key_for(entry, cache_variant(crop));
    let cached = cache::path_for(cache_dir, key);

    if cached.exists() {
        if let Ok(img) = cache::read_thumbnail(&cached) {
            return Ok(img);
        }
        // Corrupted / old-format cache entry: regenerate.
        let _ = fs::remove_file(&cached);
    }

    let mut img = decode_with_orientation(&entry.path)?;
    if let Some(aspect) = crop {
        img = center_crop_to_aspect(img, aspect);
    }
    let thumb = img.thumbnail(THUMB_MAX_DIM, THUMB_MAX_DIM);

    if let Err(e) = cache::write_thumbnail(&cached, &thumb) {
        eprintln!("glry: cache write failed: {e}");
    }

    Ok(thumb)
}

/// Variant byte mixed into the cache key so toggling crop mode doesn't return
/// a thumbnail in the wrong shape.
fn cache_variant(crop: CropAspect) -> u64 {
    match crop {
        None => 0,
        Some((w, h)) => ((w as u64) << 32) | h as u64,
    }
}

/// Center-crop `img` so its aspect matches `aspect_w:aspect_h`. If the image
/// already matches (or degenerately has a zero dimension), returns it
/// unchanged.
pub fn center_crop_to_aspect(img: DynamicImage, (aspect_w, aspect_h): (u32, u32)) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 || aspect_w == 0 || aspect_h == 0 {
        return img;
    }
    // Compare w/h vs aspect_w/aspect_h using cross-multiplication to avoid floats.
    let image_ratio = w as u64 * aspect_h as u64;
    let target_ratio = h as u64 * aspect_w as u64;
    let (crop_w, crop_h) = if image_ratio > target_ratio {
        // Image is wider than target: crop width.
        let new_w = (h as u64 * aspect_w as u64 / aspect_h as u64) as u32;
        (new_w.max(1), h)
    } else if image_ratio < target_ratio {
        // Image is taller than target: crop height.
        let new_h = (w as u64 * aspect_h as u64 / aspect_w as u64) as u32;
        (w, new_h.max(1))
    } else {
        return img;
    };
    let x = (w - crop_w) / 2;
    let y = (h - crop_h) / 2;
    DynamicImage::ImageRgba8(imageops::crop_imm(&img, x, y, crop_w, crop_h).to_image())
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

/// Load the original full-resolution image with EXIF rotation applied.
/// Used for clipboard copy where we want the unmodified source pixels.
pub fn load_original(path: &Path) -> Result<DynamicImage> {
    decode_with_orientation(path)
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
    crop: CropAspect,
}

impl ThumbWorker {
    pub fn new(
        cache_dir: PathBuf,
        picker: Arc<Picker>,
        thumb_area: Rect,
        max_full_dim: u32,
        crop: CropAspect,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            cache_dir,
            picker,
            thumb_area,
            max_full_dim,
            crop,
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
        let crop = self.crop;
        rayon::spawn(move || {
            let result = load_thumbnail(&entry, &cache_dir, crop).and_then(|img| {
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
