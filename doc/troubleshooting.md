# Troubleshooting

## Submodules Are Missing

If Cargo reports missing `framework-tui` or `img-tui`, initialize submodules:

```sh
git submodule update --init --recursive
```

## Raw Escape Sequences Appear

Force a safer render mode:

```sh
GALLERY_TUI_RENDER_MODES=symbols pdf-tui file.pdf
```

Or disable native protocol auto-detection:

```toml
[render]
auto_detect = false
```

## Pages Render Slowly

Large windows and high DPI settings produce larger PNGs and terminal streams.
Tune:

```toml
[render]
page_dpi = 180
max_concurrent = 4
preload_ahead = 4
preload_behind = 2
```

## Cache Uses Too Much Space

Lower the disk cache limit:

```toml
[render]
cache_max_bytes = 268435456
```

Lower memory cache limits:

```toml
[render]
raw_memory_cache_max_bytes = 16777216
compressed_memory_cache_max_bytes = 67108864
prepared_memory_cache_max_bytes = 67108864
```

Or clear cache:

```text
:clear-cache
```

## A Terminal Protocol Fails

Try an explicit mode order:

```sh
GALLERY_TUI_RENDER_MODES=sixel,symbols pdf-tui file.pdf
GALLERY_TUI_RENDER_MODES=kitty,symbols pdf-tui file.pdf
GALLERY_TUI_RENDER_MODES=symbols,ascii pdf-tui file.pdf
```

Check the log path printed at startup for the exact external command or render
failure.
