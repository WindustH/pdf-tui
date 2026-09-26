# Configuration

## Files

`pdf-tui` reads three files from its configuration directory:

- `config.toml`: layout, rendering, and behavior (this page)
- `keymap.toml`: key bindings ([Keymap](keymap.md))
- `theme.toml`: colors ([Theme](theme.md))

The directory is `$XDG_CONFIG_HOME/pdf-tui`, or `$HOME/.config/pdf-tui` when
`XDG_CONFIG_HOME` is unset; on Windows without either variable it is
`%APPDATA%\pdf-tui`. This applies to macOS too, so the files live in
`~/.config/pdf-tui` there as well.

Missing files are created with the defaults. When a file lacks fields (for
example after an upgrade), the missing values are filled in and the file is
rewritten; `config.toml` is written with a comment above each field. A file
that cannot be parsed, or whose active layout is invalid, is renamed to
`<name>.bak.<timestamp>` and replaced by the defaults.

Changes take effect on the next start. `:layout` and `:write-config` write
`config.toml` from a running session.

## `[layout]`

```toml
[layout]
active = "scroll"
active_args = ["2", "3"]
gap_x = 2
gap_y = 1
show_border = false
padding = 0

[layout.presets.scroll]
strategy = "scroll"
params = ["columns", "scroll_divisor"]
columns = 1
rows = 1
scroll_divisor = 1
show_border = false
padding = 0

[layout.presets.grid]
strategy = "grid"
params = ["rows", "columns"]
columns = 2
rows = 2
scroll_divisor = 1
show_border = false
padding = 0
```

| Field | Meaning |
| --- | --- |
| `active` | Preset used at startup |
| `active_args` | Arguments for the active preset, in the order of its `params` |
| `gap_x`, `gap_y` | Cells between page columns and between page rows |
| `show_border` | Draw a border around each grid page |
| `padding` | Cells between a grid page and its border |
| `presets` | Named layouts for `:layout` and the command line |

Preset fields:

| Field | Meaning |
| --- | --- |
| `strategy` | `scroll` (or `continuous`) or `grid` (or `fixed_grid`) |
| `params` | Names of the arguments `:layout <preset>` accepts, in order |
| `columns` | Page columns |
| `rows` | Page rows (grid only) |
| `scroll_divisor` | Slices per screen height (scroll only) |
| `gap_x`, `gap_y`, `show_border`, `padding` | Optional overrides of the shared values |

Parameter names that can appear in `params`: `columns` (`column`, `cols`),
`rows` (`row`), `scroll_divisor` (`divisor`, `step`, `chunk`), `gap_x`,
`gap_y`, `show_border` (`border`), and `padding` (`pad`). Counts must be between 1 and 64; larger
values in the file are clamped. Booleans accept `true`/`false`, `yes`/`no`,
`on`/`off`, and `1`/`0`. A preset with `params = ["rows", "columns"]` also
accepts `2x3` as a single argument.

Add presets under `[layout.presets.<name>]`; `scroll` and `grid` are restored
if removed. `:layout` updates `active` and `active_args`; `:layout-use` and
the layout given on the command line change only the current session.

## `[render]`

PDF rasterization:

| Field | Default | Meaning |
| --- | --- | --- |
| `pdf_raster_backend` | `"pdfium"` | `pdfium`, `mutool`, or `poppler` |
| `pdf_raster_batch_pages` | `4` | Consecutive pages rasterized by one backend run |
| `pdfium_library_path` | unset | Path to the Pdfium library or its directory; see [Rendering](rendering.md#raster-backends) |
| `pdfinfo_bin` | `"pdfinfo"` | Reads the page count and page sizes |
| `pdftoppm_bin` | `"pdftoppm"` | Poppler backend |
| `mutool_bin` | `"mutool"` | Mutool backend |
| `mutool_band_height` | `256` | `mutool draw -B` |
| `mutool_threads` | `8` | `mutool draw -T` |
| `mutool_parallel` | `true` | `mutool draw -P` |
| `pdftotext_bin` | `"pdftotext"` | Builds the search index |
| `pdftk_bin` | `"pdftk"` | Reads and writes bookmarks |
| `page_dpi` | `180` | Part of the page cache key only: pages are rasterized at the pixel size of their terminal area |

`exiftool` is always run as `exiftool` from `PATH`.

Terminal rendering:

| Field | Default | Meaning |
| --- | --- | --- |
| `auto_detect` | `true` | Detect graphics protocols, color depth, and multiplexers. When `false`, only Chafa symbols and ASCII are used |
| `chafa_bin` | `"chafa"` | Chafa executable |
| `chafa_args` | `["--format=symbols", "--colors=full", "--symbols=block", "--animate=off", "--polite=on"]` | Extra Chafa arguments. With `auto_detect`, `--colors` and `--symbols` follow the terminal; `--format`, `--probe`, `--relative`, and `--passthrough` are always set by `pdf-tui`, and `--scale=max` is added unless given |
| `chafa_threads` | `1` | `chafa --threads`; `0` lets Chafa decide |
| `zellij_sixel` | `"off"` | Sixel under Zellij: `off`, `auto` (when the terminal answers the probe), or `on` |
| `passthrough` | unset | Ignored; multiplexer passthrough is detected automatically |

Scheduling and preloading:

| Field | Default | Meaning |
| --- | --- | --- |
| `max_concurrent` | `4` | Concurrent jobs per stage (rasterizing, terminal rendering). One is kept for visible pages, so `1` disables preloading |
| `preload_ahead`, `preload_behind` | `4`, `2` | Page PNGs prepared around the view: scroll rows, grid pages, or bookmark, search, and selection entries |
| `preload_slice_ahead`, `preload_slice_behind` | `3`, `1` | Scroll rows whose slice PNGs are prepared |
| `preload_terminal_ahead`, `preload_terminal_behind` | `2`, `1` | Entries also rendered for the terminal |
| `search_preload_idle_ms` | `500` | Pause after typing a query before search previews are preloaded |

Caches (sizes in bytes):

| Field | Default | Meaning |
| --- | --- | --- |
| `cache_max_bytes` | `536870912` (512 MiB) | Disk limit for the whole cache, enforced at startup; `0` disables it |
| `cache_compression_level` | `3` | zstd level of cached terminal renders |
| `cache_compression_threads` | `2` | zstd threads; `0` compresses on one thread |
| `raw_memory_cache_max_bytes` | `33554432` (32 MiB) | Terminal renders kept in memory |
| `memory_compression` | `true` | Compress older protocol renders in memory instead of dropping them |
| `compressed_memory_cache_max_bytes` | `134217728` (128 MiB) | Compressed renders kept in memory |
| `prepared_memory_cache_max_bytes` | `134217728` (128 MiB) | Decoded images kept for protocol rendering |
| `search_highlight_cache_max_bytes` | `67108864` (64 MiB) | Disk limit for search highlight PNGs |
| `selection_cache_max_bytes` | `67108864` (64 MiB) | Disk limit for selection crop and marker PNGs |
| `selection_image_max_pixels` | `4194304` | Largest PNG copied with `Y`, in pixels |

See [Cache And Logs](cache-and-logs.md) for what each cache holds.

## `[behavior]`

| Field | Default | Meaning |
| --- | --- | --- |
| `frame_sync_navigation_viewer` | `true` | Ignore viewer navigation until the current frame has rendered |
| `frame_sync_navigation_bookmarks` | `false` | The same for bookmark navigation and its preview |
| `frame_sync_navigation_search` | `false` | The same for search results and their preview |
| `auto_refresh` | `false` | Reload the PDF when the file changes |
| `auto_refresh_poll_ms` | `500` | How often the file is checked (at least 200) |
| `auto_refresh_min_interval_ms` | `1500` | Minimum time between automatic reloads (at least 500) |
| `remember_reading_position` | `false` | Reopen each document where it was last closed |
| `bookmarks_left_ratio`, `bookmarks_right_ratio` | `2`, `1` | Width ratio of the bookmark tree and preview panels |
| `search_left_ratio`, `search_right_ratio` | `2`, `1` | Width ratio of the search result and preview panels |
| `scroll_lines` | `4` | Unused; kept for compatibility |

Frame-synced navigation keeps fast key repeat from skipping past pages that
never finished drawing; turn it off for free-running navigation.

With `auto_refresh`, a change to the file (size or modification time) is
noticed within one poll interval and triggers a reload, at most once per
`auto_refresh_min_interval_ms`; the reading position is kept.

`remember_reading_position` stores positions in `progress.toml` in the cache
directory when `pdf-tui` exits, keyed by the file's path, size, and
modification time: a PDF changed by another program opens at the start, while
changes seen during the session (refresh, metadata or bookmark edits) are
remembered. `--progress` takes precedence, and `:clear-cache` keeps the
positions. The 100 most recently closed documents are kept.
