# Quick Start

For native Windows, use the
[experimental Windows setup guide](windows.md). The Windows version is built by
CI but has not been tested by the maintainer.

## Install Dependencies

```sh
sudo pacman -S poppler chafa perl-image-exiftool pdftk mupdf-tools
yay -S pdfium-binaries-bin
brew install poppler chafa exiftool pdftk-java mupdf
```

- `poppler` provides `pdfinfo` (required to open a document), `pdftotext`
  (search and copying selected text), and the optional Poppler raster backend.
- `pdfium` is the default raster backend; `mupdf` provides the optional
  `mutool` backend.
- `chafa` renders pages as text when the terminal has no graphics protocol.
- `exiftool` shows and edits metadata; `pdftk` shows and edits bookmarks. On
  Homebrew, `pdftk` comes from `pdftk-java`.
- Copying a selection uses `wl-copy` on Wayland, `xclip` or `xsel` on X11, and
  `pbcopy`/`osascript` on macOS.

Without the optional tools `pdf-tui` still opens documents; the affected view
reports what is missing.

The Homebrew formula bundles a compatible Pdfium library and launches
`pdf-tui` with `PDF_TUI_PDFIUM_LIBRARY_PATH` set. Source builds on macOS need a
compatible `libpdfium.dylib` through `PDF_TUI_PDFIUM_LIBRARY_PATH` or
`render.pdfium_library_path`.

## Build From Source

```sh
git clone --recurse-submodules <pdf-tui-repo-url>
cd pdf-tui
cargo run --release -- /path/to/file.pdf
```

In an existing clone, fetch the submodules with
`git submodule update --init --recursive`.

## Run

```sh
pdf-tui /path/to/file.pdf
pdf-tui --progress 3.25 /path/to/file.pdf
pdf-tui /path/to/file.pdf scroll 1 3
pdf-tui /path/to/file.pdf grid 2x3
```

- `--progress` is a 0-based page position: `0` is the start of the first page
  and `3.25` a quarter into the fourth. See [Views](views.md#reading-progress).
- Arguments after the file select a layout preset for this session, with the
  same syntax as [`:layout`](commands.md#layout-name-args).
- `pdf-tui --help` lists the options; `--version` prints the version.

The first run creates `config.toml`, `keymap.toml`, and `theme.toml` in
`~/.config/pdf-tui/` (or `$XDG_CONFIG_HOME/pdf-tui/`) and stores cache and
logs in `~/.cache/pdf-tui/` (or `$XDG_CACHE_HOME/pdf-tui/`). The log path is
printed when the program starts.

## Basic Workflow

1. Scroll with `j`/`k`, the arrow keys, or the mouse wheel; `h`/`l` and
   PgUp/PgDn move a screen at a time; `gg` and `G` jump to the first and last
   page.
2. Press `F1` to list the keys of the current view.
3. Press `:` for a command: `:layout-use scroll 2 3` changes the layout for
   this session, `:layout scroll 2 3` also saves it.
4. Press `r` or run `:refresh` after regenerating the PDF, or enable
   `behavior.auto_refresh`.
5. Press `m` to inspect metadata, then `e` to edit it.
6. Press `b` to browse bookmarks, `Enter` to jump, and `e` to edit them.
7. Press `s` to search embedded text; `Tab` moves through results and
   `Enter` jumps to one.
8. Click two corners of a page region to select it, then press `v` for the
   selection view, where `y` copies its text and `Y` a PNG.
9. Press `q` to quit.
