//! User configuration. Reads a theme file from the platform config dir
//! (`~/.config/glry/config` on Linux) using a minimal `key = value` syntax.
//!
//! Values are ratatui color strings: a named color (`red`, `darkgray`, …),
//! an 8-bit index (`0`–`255`), or `#rrggbb`.

use anyhow::{Context, Result};
use ratatui::style::Color;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Resolved color palette used everywhere in the UI.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub header_fg: Color,
    pub header_bg: Color,
    pub selection_fg: Color,
    pub selection_bg: Color,
    pub status_fg: Color,
    pub status_bg: Color,
    pub directory_fg: Color,
    pub error_fg: Color,
    pub loading_fg: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            header_fg: Color::Black,
            header_bg: Color::Cyan,
            selection_fg: Color::Black,
            selection_bg: Color::Cyan,
            status_fg: Color::Gray,
            status_bg: Color::Black,
            directory_fg: Color::Yellow,
            error_fg: Color::Red,
            loading_fg: Color::DarkGray,
        }
    }
}

/// Returns `~/.config/glry/config`, whether or not the file exists.
pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("glry").join("config"))
}

/// Contents written to a fresh config file on first run. All entries are
/// commented so the built-in defaults apply until the user opts in.
const DEFAULT_CONFIG: &str = "\
# glry configuration — uncomment a line to override the default.
#
# Values are ratatui color strings: a named color (black, red, darkgray, …),
# an 8-bit index (0-255), or \"#rrggbb\" hex.

# header_fg    = \"black\"
# header_bg    = \"cyan\"
# selection_fg = \"black\"
# selection_bg = \"cyan\"
# status_fg    = \"gray\"
# status_bg    = \"black\"
# directory_fg = \"yellow\"
# error_fg     = \"red\"
# loading_fg   = \"darkgray\"
";

/// Load theme from `~/.config/glry/config`. If the file is missing, write a
/// commented template and continue with defaults. Parse/unknown-key errors
/// are reported to stderr and the bad entry is skipped.
pub fn load() -> Theme {
    let Some(path) = config_path() else {
        return Theme::default();
    };
    match load_from(&path) {
        Ok(theme) => theme,
        Err(e) => {
            eprintln!("glry: config {}: {e:#}", path.display());
            Theme::default()
        }
    }
}

fn load_from(path: &Path) -> Result<Theme> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Err(e) = write_default(path) {
                eprintln!("glry: could not write default config: {e:#}");
            }
            return Ok(Theme::default());
        }
        Err(e) => {
            return Err(e).with_context(|| format!("reading {}", path.display()));
        }
    };

    let mut theme = Theme::default();
    for (lineno, raw) in text.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            eprintln!(
                "glry: {}:{}: expected `key = value`",
                path.display(),
                lineno + 1,
            );
            continue;
        };
        let key = key.trim();
        let value = unquote(value.trim());
        let color = match Color::from_str(value) {
            Ok(c) => c,
            Err(_) => {
                eprintln!(
                    "glry: {}:{}: invalid color `{value}` for `{key}`",
                    path.display(),
                    lineno + 1,
                );
                continue;
            }
        };
        match key {
            "header_fg" => theme.header_fg = color,
            "header_bg" => theme.header_bg = color,
            "selection_fg" => theme.selection_fg = color,
            "selection_bg" => theme.selection_bg = color,
            "status_fg" => theme.status_fg = color,
            "status_bg" => theme.status_bg = color,
            "directory_fg" => theme.directory_fg = color,
            "error_fg" => theme.error_fg = color,
            "loading_fg" => theme.loading_fg = color,
            _ => eprintln!(
                "glry: {}:{}: unknown key `{key}`",
                path.display(),
                lineno + 1,
            ),
        }
    }
    Ok(theme)
}

fn write_default(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, DEFAULT_CONFIG)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

fn unquote(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &s[1..s.len() - 1];
        }
    }
    s
}
