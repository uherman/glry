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
| `Esc` / `q`              | Exit fullscreen, or quit            |
| `Ctrl-C`                 | Quit                                |

In fullscreen, `h` / `l` (or arrows) step to the previous / next image.

## Supported formats

JPEG, PNG, GIF, BMP, ICO, TIFF, WebP, AVIF, PNM/PBM/PGM/PPM, TGA, DDS, FarbFeld,
QOI, HDR, EXR — whatever the [`image`](https://crates.io/crates/image) crate
decodes.

## Cache

Thumbnails are written to `~/.cache/glry/` (or the platform equivalent) as PNGs
named by a 64-bit xxh3 of `(path, size, mtime)`. Safe to delete at any time;
glry will regenerate on next view.

## License

MIT
