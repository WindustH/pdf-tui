# pdf-tui

`pdf-tui` is a terminal PDF reader built with rataui.

https://github.com/user-attachments/assets/bad500d9-49e9-4634-b381-4949f8f0a255

Runtime dependencies:

- `poppler` for `pdfinfo`, `pdftotext`, and the optional Poppler raster backend
- `pdfium` for the default PDF raster backend
- `mupdf` for the optional `mutool` raster backend
- `chafa` for terminal symbol rendering fallbacks
- `exiftool` for editing PDF metadata
- `pdftk` for reading and editing PDF bookmarks. Homebrew provides this
  command through the `pdftk-java` formula.

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

The Homebrew stable formula downloads a prebuilt release binary. To build the
latest git version from source:

```sh
brew install --HEAD WindustH/tap/pdf-tui
```

## Usage

```sh
pdf-tui /path/to/file.pdf
pdf-tui --progress 0.0 /path/to/file.pdf
pdf-tui /path/to/file.pdf scroll 1 3
pdf-tui /path/to/file.pdf scroll 2 3
pdf-tui /path/to/file.pdf grid 2 3
```

Runtime commands are available with `:`:

```text
layout scroll <columns> <scroll_divisor>
layout grid <rows> <columns>
refresh
metadata
bookmarks
search
help
clear-cache
quit
```

## Documentation
[doc/index.md](doc/index.md).
