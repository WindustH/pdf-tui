# Cache And Logs

Cache and logs are stored under:

- `~/.cache/pdf-tui/`

Important subdirectories:

- `pages/`: PDF page and slice PNG cache
- `render/`: compressed terminal render stream cache
- `text/`: compressed embedded-text search index cache
- `search-highlight/`: search preview highlight PNG cache
- `selection/`: selection anchor marker PNGs and final selection crop PNGs
- `logs/latest.log`: log file for the latest run

## Cache Cleanup

Runtime rendering uses a multi-level cache:

- L1: raw rendered terminal streams in memory
- L2: compressed rendered terminal streams in memory
- L3: compressed search indexes plus page PNG and terminal stream files on disk
- L4: cache miss path that regenerates data from the PDF or PNG

L4 is not a stored cache and has no size setting.

Preloading feeds those cache levels by distance. Farther candidates warm page
PNGs on disk, nearer scroll candidates warm slice PNGs, and the nearest
candidates warm terminal streams in the render cache and memory cache. Visible
work always has higher scheduler priority than queued preloads.

The cache limits are configured with:

```toml
[render]
raw_memory_cache_max_bytes = 33554432
compressed_memory_cache_max_bytes = 134217728
prepared_memory_cache_max_bytes = 134217728
cache_max_bytes = 536870912
```

When L1 exceeds its limit, cold protocol streams are compressed into L2 when
`memory_compression` is enabled. When L2 or L3 exceeds its limit, older entries
are removed. L3 uses LRU marker files on disk.

Clear cache from inside the TUI:

```text
:clear-cache
```

This clears cached page PNGs, rendered terminal streams, search text indexes,
search highlight PNGs, selection PNGs, and LRU marker files. It does not delete
logs.

Selection previews and `Y` copies cache only final cropped selection PNGs.
Poppler and Pdfium can render that crop directly from the PDF. Mutool falls
back through a temporary full-page render because the installed `mutool draw`
CLI does not expose a reliable crop rectangle option; that temporary page is
written under the temporary work directory and is not kept in the page cache.

## Logs

`pdf-tui` prints the active log path to stderr at startup. Startup overwrites
`logs/latest.log` and removes older `*.log` files, so only the latest run is
kept. Logs include PDF page rendering, terminal render requests, cache
hits/misses, preload behavior, and failures from external tools such as
`pdfinfo`, the selected PDF raster backend, or Chafa.
