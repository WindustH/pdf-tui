# pdf-tui Documentation

This directory contains user, configuration, and implementation notes for
`pdf-tui`.

## Start Here

- [Quick Start](quick-start.md): build, run, and open a PDF.
- [Controls](controls.md): default browsing, metadata, bookmark, search, prompt, mouse, and which-key controls.
- [Commands](commands.md): command prompt commands such as `:layout`, `:layout-use`, and `:clear-cache`.

## Configuration

- [Configuration](configuration.md): config files and `config.toml` fields.
- [Keymap](keymap.md): context-aware keymap format.
- [Theme](theme.md): colors, status line, completion list, and which-key styling.

## Features

- [Views](views.md): scroll, grid, metadata, bookmark, and search view behavior.
- [Rendering](rendering.md): PDF rasterization, terminal image rendering, preloading, and caching.
- [Terminal Graphics](terminal-graphics.md): Kitty, Sixel, iTerm2, Chafa symbols, and ASCII fallback.
- [Cache And Logs](cache-and-logs.md): cache directories, zstd render cache, clear-cache, and logs.

## Development

- [Architecture](architecture.md): module boundaries and dependency structure.
- [Troubleshooting](troubleshooting.md): common display, cache, and terminal graphics issues.
