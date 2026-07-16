# Quick Start

Clone the repository with submodules:

```sh
git clone --recurse-submodules <pdf-tui-repo-url>
```

If the repository is already cloned:

```sh
git submodule update --init --recursive
```

Run from source:

```sh
cargo run --release -- /path/to/file.pdf
```

Or run the binary directly:

```sh
pdf-tui /path/to/file.pdf
```

Open at a 0-based reading progress:

```sh
pdf-tui --progress 3.25 /path/to/file.pdf
```

Use a startup layout override:

```sh
pdf-tui /path/to/file.pdf scroll 1 3
pdf-tui /path/to/file.pdf scroll 2 3
pdf-tui /path/to/file.pdf grid 2 3
```

Default configuration files are created on first run:

- `~/.config/pdf-tui/config.toml`
- `~/.config/pdf-tui/keymap.toml`
- `~/.config/pdf-tui/theme.toml`

Cache and logs are stored under:

- `~/.cache/pdf-tui/`

Basic workflow:

1. Move with `j/k`, arrow keys, page keys, or mouse wheel.
2. Press `:` to enter a command.
3. Use `:layout-use scroll 2 3` for a temporary layout change.
4. Use `:layout scroll 2 3` to save the layout to `config.toml`.
5. Press `r` or run `:refresh` after regenerating the PDF.
6. Press `m` to inspect metadata, then `e` to edit supported PDF metadata.
7. Press `b` to inspect bookmarks, then `e` to edit PDF bookmarks.
8. Press `q` to quit.
