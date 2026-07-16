# Architecture

`pdf-tui` is built on:

- `framework-tui` for key dispatch, prompt editing, command history, completion selection, and footer widgets
- `img-tui` for terminal capability detection, native image rendering helpers, and protocol overlay frame management

Both dependencies are git submodules under:

- `crates/framework-tui`
- `crates/img-tui`

Initialize them with:

```sh
git submodule update --init --recursive
```

## Main Modules

- `main.rs`: startup, terminal capability detection, event loop, async event handling
- `terminal.rs`: Ratatui terminal setup and `img-tui` protocol frame renderer integration
- `app/`: application state, input handling, navigation, and progress mapping
- `config/`: config loading plus layout, render, behavior, keymap, and theme submodules
- `pdf/`: PDF metadata, page/slice rasterization, and page preload store
- `metadata.rs`: PDF metadata read/edit/write support through `exiftool`
- `bookmarks.rs`: PDF bookmark read/edit/write support through `pdftk`
- `render/`: terminal rendering state machine, cache file codec, Chafa driver, native protocol driver, cache keys
- `ui/`: frame composition plus footer, scroll, grid, page drawing, and preload triggers
- `layout.rs`: scroll/grid geometry and progress-relevant layout calculations
- `cache.rs`: cache cleanup and clear-cache support

## Input

The command prompt uses `framework-tui::handle_prompt_key` and
`framework-tui::handle_prompt_paste`. `pdf-tui` supplies command-specific
completion candidates and command execution.

External metadata and bookmark editing use `framework-tui::edit_text_in_editor`;
the TUI temporarily suspends alternate-screen/raw-mode state before launching
`$EDITOR` and restores protocol image state when returning.

## Refresh

Manual `:refresh`, the viewer `r` key, and optional automatic refresh all share
the same reload path. A reload replaces the `PdfDocument`, refreshes metadata
and bookmarks, clears in-memory page and terminal render state, then re-applies
the current reading progress to the new document.

Automatic refresh is a lightweight background polling thread. It watches the
opened file signature and sends refresh requests with a configurable minimum
interval so frequent file writes collapse into bounded reloads.

## Rendering

The render pipeline is deliberately split:

- PDF rasterization is owned by `pdf/`
- terminal stream conversion is owned by `render/`
- protocol frame lifetime is owned by `img-tui`
- visible placement and redraw decisions are owned by `ui/`

This separation keeps PDF page cache, terminal stream cache, and terminal
overlay state from being mixed together.
