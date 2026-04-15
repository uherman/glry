//! On-disk thumbnail cache.
//!
//! Cache key is derived from the canonical source path + mtime + size. Files are
//! written atomically (temp + rename) to prevent partial reads after a crash.
//! Lookup is by filename only; no manifest/index file.

use anyhow::{Context, Result};
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

/// Path where the cached PNG for `key` lives (file may or may not exist).
pub fn path_for(cache_dir: &Path, key: u64) -> PathBuf {
    cache_dir.join(format!("{:016x}.png", key))
}

/// Atomically write `data` to `dest` (write to dest.tmp, then rename).
pub fn atomic_write(dest: &Path, data: &[u8]) -> Result<()> {
    let tmp = dest.with_extension("png.tmp");
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
