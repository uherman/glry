//! Directory scanning. Produces a Vec<Entry> for the given directory:
//! a "Parent" entry first if not at the filesystem root, then directories,
//! then image files (sorted alphabetically within each group).
//!
//! PDFs are included in the image list. They are rasterized on demand by
//! [`crate::thumbnail`] via the `pdftoppm` helper (from poppler-utils), which
//! must be on `PATH` at runtime for PDFs to render.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct ImageEntry {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub modified: SystemTime,
}

#[derive(Debug, Clone)]
pub enum Entry {
    Parent(PathBuf),
    SubDir { path: PathBuf, name: String },
    Image(ImageEntry),
}

impl Entry {
    pub fn display_name(&self) -> &str {
        match self {
            Entry::Parent(_) => "..",
            Entry::SubDir { name, .. } => name,
            Entry::Image(img) => &img.name,
        }
    }
}

const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "ico", "tif", "tiff", "webp", "avif", "pnm", "pbm", "pgm",
    "ppm", "tga", "dds", "ff", "qoi", "hdr", "exr", "pdf",
];

fn is_image_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let lower = e.to_ascii_lowercase();
            IMAGE_EXTENSIONS.iter().any(|ext| *ext == lower)
        })
        .unwrap_or(false)
}

/// True if `path` has a `.pdf` extension (case-insensitive). PDFs live in the
/// same entry list as images but go through a separate rasterization path in
/// [`crate::thumbnail`].
pub fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

/// Scan `dir` and return entries: ".." (if applicable), subdirs, then images.
pub fn scan(dir: &Path) -> Result<Vec<Entry>> {
    let dir = dir
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", dir.display()))?;
    let read = fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))?;

    let mut subdirs: Vec<(PathBuf, String)> = Vec::new();
    let mut images: Vec<ImageEntry> = Vec::new();

    for entry in read.flatten() {
        let path = entry.path();
        let name = match entry.file_name().to_str() {
            Some(n) => n.to_owned(),
            None => continue, // skip non-UTF8 names
        };
        if name.starts_with('.') {
            continue; // skip hidden entries
        }

        // Use symlink_metadata for directories to avoid following symlink cycles,
        // but follow symlinks for regular files (so a symlinked image still resolves).
        let lmeta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };

        if lmeta.is_dir() {
            subdirs.push((path, name));
        } else if is_image_ext(&path) {
            // Follow symlink for files via fs::metadata.
            let meta = match fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if !meta.is_file() {
                continue;
            }
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            images.push(ImageEntry {
                path,
                name,
                size: meta.len(),
                modified,
            });
        }
    }

    subdirs.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    images.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let mut out: Vec<Entry> = Vec::with_capacity(subdirs.len() + images.len() + 1);
    if let Some(parent) = dir.parent() {
        out.push(Entry::Parent(parent.to_path_buf()));
    }
    for (path, name) in subdirs {
        out.push(Entry::SubDir { path, name });
    }
    for img in images {
        out.push(Entry::Image(img));
    }
    Ok(out)
}
