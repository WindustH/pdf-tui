# Configuration

Default configuration files are created on first run:

- `~/.config/pdf-tui/config.toml`
- `~/.config/pdf-tui/keymap.toml`
- `~/.config/pdf-tui/theme.toml`

When existing configuration files are missing fields introduced by a newer
version, `pdf-tui` normalizes them and writes the parsed defaults back.

## `config.toml`

Top-level tables:

- `[layout]`
- `[render]`
- `[behavior]`

## `[layout]`

Layout has one active preset plus shared style fields:

- `active`: preset name to use at startup
- `active_args`: optional positional arguments for the active preset
- `gap_x`, `gap_y`: spacing between grid cells or scroll rows
- `show_border`: show or hide page borders
- `padding`: content padding inside page frames
- `presets`: named layouts available to `:layout`

Default presets:

```toml
[layout.presets.scroll]
strategy = "scroll"
params = ["columns", "scroll_divisor"]
columns = 1
rows = 1
scroll_divisor = 3
show_border = false
padding = 0

[layout.presets.grid]
strategy = "grid"
params = ["rows", "columns"]
columns = 2
rows = 2
scroll_divisor = 3
show_border = true
padding = 1
```

Supported preset fields:

- `strategy`: `scroll` or `grid`
- `params`: positional parameter names accepted by `:layout`
- `columns`: page columns
- `rows`: grid rows
- `scroll_divisor`: scroll slice divisor
- `gap_x`, `gap_y`: optional preset-specific spacing overrides
- `show_border`: optional preset-specific border override
- `padding`: optional preset-specific padding override

Running `:layout` updates `active` and `active_args` in this file. Running
`:layout-use` changes only the current session.

## `[render]`

Render fields:

- `pdfinfo_bin`: `pdfinfo` executable
- `pdftoppm_bin`: `pdftoppm` executable
- `pdftoppm_batch_pages`: maximum consecutive pages rendered by one `pdftoppm` process
- `pdftk_bin`: `pdftk` executable, used for reading and writing PDF bookmarks
- `pdftotext_bin`: `pdftotext` executable, used for embedded text search
- `page_dpi`: base PDF rasterization DPI
- `chafa_bin`: Chafa executable
- `auto_detect`: detect terminal graphics support
- `chafa_args`: extra Chafa fallback arguments
- `raw_memory_cache_max_bytes`: L1 raw rendered terminal stream memory limit
- `compressed_memory_cache_max_bytes`: L2 compressed rendered terminal stream memory limit
- `prepared_memory_cache_max_bytes`: prepared native image memory limit
- `search_highlight_cache_max_bytes`: search preview highlight PNG cache limit
- `memory_compression`: keep cold rendered terminal streams compressed in memory
- `cache_max_bytes`: L3 disk cache size limit for page PNGs, text indexes, and terminal streams
- `cache_compression_level`: L3 zstd compression level
- `cache_compression_threads`: L3 zstd compression threads
- `max_concurrent`: maximum concurrent page/render tasks
- `chafa_threads`: Chafa threads per process
- `preload_ahead`, `preload_behind`: preload window around the visible region
- `passthrough`: terminal multiplexer passthrough override
- `zellij_sixel`: `off`, `auto`, or `on`

`auto_detect` uses `img-tui` terminal probing to choose Kitty, Sixel, iTerm2,
Chafa symbols, or ASCII fallback.

## `[behavior]`

Behavior fields:

- `scroll_lines`: retained for keyboard scroll compatibility
- `auto_refresh`: enable a background watcher for the opened PDF
- `auto_refresh_poll_ms`: file change polling interval
- `auto_refresh_min_interval_ms`: minimum interval between automatic refresh requests
- `bookmarks_left_ratio`: left bookmarks panel ratio
- `bookmarks_right_ratio`: right preview panel ratio
- `search_left_ratio`: left search panel ratio
- `search_right_ratio`: right preview panel ratio

Automatic refresh is disabled by default. When enabled, `pdf-tui` watches the
opened PDF file signature and requests a refresh after updates. Repeated updates
are rate limited by `auto_refresh_min_interval_ms`.

The default bookmarks and search panel ratio is `2:1`.
