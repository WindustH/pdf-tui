# pdf-tui

`pdf-tui` is a terminal PDF reader built with `framework-tui` and `img-tui`.

Dependencies are managed as git submodules:

```sh
git submodule update --init --recursive
```

Runtime dependencies:

- `poppler` for `pdfinfo` and `pdftoppm`
- `chafa` for terminal symbol rendering fallbacks
- `exiftool` for editing PDF metadata

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
help
clear-cache
quit
```

Full documentation is in [doc/index.md](doc/index.md).
