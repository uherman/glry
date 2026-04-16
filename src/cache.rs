//! On-disk thumbnail cache.
//!
//! Cache key is derived from the canonical source path + mtime + size. Files are
//! written atomically (temp + rename) to prevent partial reads after a crash.
//! Lookup is by filename only; no manifest/index file.
//!
//! Thumbnails are stored as raw RGBA pixel data (8 bytes header + pixels) for
//! maximum read/write speed — no PNG encode/decode overhead.

use anyhow::{Context, Result, bail};
use image::DynamicImage;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use xxhash_rust::xxh3::Xxh3;

use crate::scan::ImageEntry;

/// Returns `~/.cache/glry/`, creating it if it does not exist.
pub fn cache_dir() -> Result<PathBuf> {
    let base = dirs::cache_dir().context("could not locate user cache directory")?;
    let dir = base.join("glry");
    fs::create_dir_all(&dir).with_context(|| format!("creating cache dir {}", dir.display()))?;
    Ok(dir)
}

/// Compute a stable cache key for an image entry.
pub fn key_for(entry: &ImageEntry) -> u64 {
    let mut h = Xxh3::new();
    h.update(entry.path.as_os_str().as_encoded_bytes());
    h.update(&entry.size.to_le_bytes());
    let mtime_ns = entry
        .modified
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    h.update(&mtime_ns.to_le_bytes());
    h.digest()
}

/// Path where the cached thumbnail for `key` lives (file may or may not exist).
pub fn path_for(cache_dir: &Path, key: u64) -> PathBuf {
    cache_dir.join(format!("{:016x}.raw", key))
}

/// Write a thumbnail as raw RGBA pixel data: `[u32 width][u32 height][RGBA bytes]`.
pub fn write_thumbnail(dest: &Path, img: &DynamicImage) -> Result<()> {
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    let pixel_bytes = rgba.as_raw();
    let mut data = Vec::with_capacity(8 + pixel_bytes.len());
    data.extend_from_slice(&w.to_le_bytes());
    data.extend_from_slice(&h.to_le_bytes());
    data.extend_from_slice(pixel_bytes);
    atomic_write(dest, &data)
}

/// Read a cached thumbnail from raw RGBA pixel data.
pub fn read_thumbnail(path: &Path) -> Result<DynamicImage> {
    let data = fs::read(path).with_context(|| format!("reading cache {}", path.display()))?;
    if data.len() < 8 {
        bail!("cache file too small");
    }
    let w = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let h = u32::from_le_bytes(data[4..8].try_into().unwrap());
    let expected = 8 + (w as usize) * (h as usize) * 4;
    if data.len() != expected {
        bail!(
            "cache size mismatch: expected {expected}, got {}",
            data.len()
        );
    }
    let buf = image::ImageBuffer::from_raw(w, h, data[8..].to_vec())
        .context("invalid image dimensions in cache")?;
    Ok(DynamicImage::ImageRgba8(buf))
}

/// Atomically write `data` to `dest` (write to temp, then rename).
pub fn atomic_write(dest: &Path, data: &[u8]) -> Result<()> {
    let tmp = dest.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)
            .with_context(|| format!("creating temp file {}", tmp.display()))?;
        f.write_all(data)
            .with_context(|| format!("writing temp file {}", tmp.display()))?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, dest)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), dest.display()))?;
    Ok(())
}
