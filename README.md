# pdf-tui

`pdf-tui` is a PDF reader for the terminal, built with ratatui.

https://github.com/user-attachments/assets/bad500d9-49e9-4634-b381-4949f8f0a255

- Sharp page images through the Kitty, Sixel, or iTerm2 graphics protocols,
  with Chafa text rendering as a fallback in any terminal
- Continuous scrolling in one or more columns, or a fixed page grid
- Bookmark tree with page previews, embedded-text search with highlighted
  matches, and mouse selection of page regions to copy as text or PNG
- Edit PDF metadata and bookmarks in `$EDITOR`
- Reload with `r`, or automatically whenever the PDF is rebuilt, and reopen
  documents where you left off (both opt-in in `config.toml`)
- Pages are rendered in the background, preloaded around the current
  position, and cached on disk

## Dependencies

- `poppler` for `pdfinfo` (required), `pdftotext` (search and selection
  text), and the optional Poppler raster backend
- `pdfium` for the default PDF raster backend
- `mupdf` for the optional `mutool` raster backend
- `chafa` for text rendering when no graphics protocol is available
- `exiftool` for showing and editing PDF metadata
- `pdftk` for reading and editing PDF bookmarks. Homebrew provides this
  command through the `pdftk-java` formula.
- optional `wl-copy`, `xclip`, or `xsel` on Linux for copying selected text or
  PNGs

Manual dependency install examples:

```sh
sudo pacman -S poppler chafa perl-image-exiftool pdftk mupdf-tools
yay -S pdfium-binaries-bin
brew install poppler chafa exiftool pdftk-java mupdf
```

The Homebrew formula bundles a compatible Pdfium dynamic library and launches
`pdf-tui` with `PDF_TUI_PDFIUM_LIBRARY_PATH` set, so the default Pdfium backend
works without editing the config. When running from source on macOS, provide a
compatible `libpdfium.dylib` with `PDF_TUI_PDFIUM_LIBRARY_PATH` or
`render.pdfium_library_path`.

## Installation

Arch Linux AUR:

```sh
yay -S pdf-tui-bin
```

Alternative AUR packages:

```sh
yay -S pdf-tui      # build the latest stable release from source
yay -S pdf-tui-git  # build the latest git version from source
```

Homebrew:

```sh
brew install WindustH/tap/pdf-tui
```

Windows (experimental and untested): download the
`x86_64-pc-windows-msvc` ZIP from the release page, then follow the
[Windows setup guide](doc/windows.md). The Windows build is produced by CI, but
has not been tested by the maintainer on a real Windows installation. The guide
covers Poppler and optional feature dependencies plus the default Pdfium
backend and its `pdfium.dll` configuration. Pdfium was the fastest backend in
the project's existing Linux benchmark; Windows performance is untested.

The Homebrew stable formula downloads a prebuilt release binary. To build the
latest git version from source:

```sh
brew install --HEAD WindustH/tap/pdf-tui
```

## Usage

```sh
pdf-tui /path/to/file.pdf
pdf-tui --progress 12.5 /path/to/file.pdf # 0-based: the middle of page 13
pdf-tui /path/to/file.pdf scroll 2 3      # two columns, three steps per screen
pdf-tui /path/to/file.pdf grid 2 3        # two rows of three pages
```

Scroll with `j`/`k` or the mouse wheel, move a screen at a time with `h`/`l`,
and press `F1` for the key bindings of the current view. `b` opens bookmarks, `s`
searches, `m` shows metadata, and `:` opens the command prompt, e.g.
`:layout-use grid 2 2`. `q` quits.

## Documentation

- [Documentation index](doc/index.md)
- [Quick start](doc/quick-start.md)
- [Controls](doc/controls.md) and [commands](doc/commands.md)
- [Configuration](doc/configuration.md)
- [Troubleshooting](doc/troubleshooting.md)
- [Windows setup (experimental and untested)](doc/windows.md)
