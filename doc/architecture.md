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
- `render/`: terminal rendering state machine, cache file codec, Chafa driver, native protocol driver, cache keys
- `ui/`: frame composition plus footer, scroll, grid, page drawing, and preload triggers
- `layout.rs`: scroll/grid geometry and progress-relevant layout calculations
- `cache.rs`: cache cleanup and clear-cache support

## Input

The command prompt uses `framework-tui::handle_prompt_key` and
`framework-tui::handle_prompt_paste`. `pdf-tui` supplies command-specific
completion candidates and command execution.

## Rendering

The render pipeline is deliberately split:

- PDF rasterization is owned by `pdf/`
- terminal stream conversion is owned by `render/`
- protocol frame lifetime is owned by `img-tui`
- visible placement and redraw decisions are owned by `ui/`

This separation keeps PDF page cache, terminal stream cache, and terminal
overlay state from being mixed together.
