# glry

A terminal image gallery. Browse a directory tree of images from inside a TUI,
with thumbnails rendered directly in the terminal.

Built on [ratatui] and [ratatui-image]; uses your terminal's graphics protocol
(Kitty / iTerm2 / Sixel, auto-detected) to draw real pixels.

[ratatui]: https://github.com/ratatui-org/ratatui
[ratatui-image]: https://github.com/benjajaja/ratatui-image

## Features

- **Grid and list views** — toggle with `Tab`.
- **Fullscreen viewer** — `Enter` on an image; arrows step between images.
- **Vim-style keys** — `hjkl`, `gg`, `G`, `q`.
- **Directory navigation** — `..` entry and `Backspace` to go up.
- **EXIF orientation** — images are rotated according to their EXIF tag.
- **On-disk thumbnail cache** at `~/.cache/glry/`, keyed by path + size + mtime.
- **Parallel decode** via rayon; the UI stays responsive while thumbnails load.

## Install

Requires a recent Rust toolchain (edition 2024).

```sh
cargo install --path .
```

Or run directly from the repo:

```sh
cargo run --release -- /path/to/photos
```

## Usage

```sh
glry [PATH]
```

`PATH` defaults to the current directory.

Your terminal must support an inline-graphics protocol. Known to work in
Kitty, WezTerm, Ghostty, iTerm2, and any Sixel-capable terminal.

On Linux, the `y` (copy-to-clipboard) key shells out to a helper that owns the
clipboard selection after glry exits. Install the one matching your session:

- **Wayland:** `wl-clipboard` (provides `wl-copy`)
- **X11:** `xclip`

macOS and Windows work out of the box.

### Keys

| Key                      | Action                              |
| ------------------------ | ----------------------------------- |
| `h` `j` `k` `l` / arrows | Move selection                      |
| `g g` / `Home`           | First entry                         |
| `G` / `End`              | Last entry                          |
| `PgUp` / `PgDn`          | Page up / down                      |
| `Tab`                    | Toggle grid / list view             |
| `Enter`                  | Open image fullscreen, or enter dir |
| `Backspace`              | Go to parent directory              |
| `y`                      | Copy selected image to clipboard    |
| `Esc` / `q`              | Exit fullscreen, or quit            |
| `Ctrl-C`                 | Quit                                |

In fullscreen, `h` / `l` (or arrows) step to the previous / next image, `b`
toggles the header / status bars, and `c` toggles fill mode — the image is
cropped to the terminal's aspect ratio so it fills edge-to-edge, trimming only
what's needed.

## Supported formats

JPEG, PNG, GIF, BMP, ICO, TIFF, WebP, AVIF, PNM/PBM/PGM/PPM, TGA, DDS, FarbFeld,
QOI, HDR, EXR — whatever the [`image`](https://crates.io/crates/image) crate
decodes.

## Configuration

glry reads an optional config file from `~/.config/glry/config` (or the
platform equivalent on macOS/Windows). On first run glry writes a commented
template there with the defaults shown; uncomment any line to override.
Unknown keys and bad values are reported on stderr and skipped.

Format is `key = value`, one per line, with `#` for comments. Color values
are ratatui color strings: a named color (`black`, `red`, `darkgray`, …), an
8-bit index (`0`–`255`), or a `#rrggbb` hex code. Boolean values are `true`
or `false`.

```ini
# ~/.config/glry/config
header_fg    = "black"
header_bg    = "cyan"
selection_fg = "black"
selection_bg = "cyan"
status_fg    = "gray"
status_bg    = "black"
directory_fg = "yellow"
error_fg     = "red"
loading_fg   = "darkgray"

# Center-crop grid thumbnails to the cell aspect so every cell is filled.
# Default is true; set to false to letterbox each image inside its cell.
thumbnail_crop = true

# Hide the header and status bars when opening the fullscreen viewer.
# The `b` key always toggles them; this just sets the initial state.
fullscreen_hide_bars = false
```

## Cache

Thumbnails are written to `~/.cache/glry/` (or the platform equivalent) as raw
RGBA files named by a 64-bit xxh3 of `(path, size, mtime, crop-variant)`. Safe
to delete at any time; glry will regenerate on next view. Changing
`thumbnail_crop` produces a distinct cache entry so the old shape isn't reused.

## License

MIT
