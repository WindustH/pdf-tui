# Cache And Logs

Cache and logs are stored under:

- `~/.cache/pdf-tui/`

Important subdirectories:

- `pages/`: PDF page and slice PNG cache
- `render/`: compressed terminal render stream cache
- `text/`: compressed embedded-text search index cache
- `search-highlight/`: search preview highlight PNG cache
- `logs/latest.log`: log file for the latest run

## Cache Cleanup

Runtime rendering uses a multi-level cache:

- L1: raw rendered terminal streams in memory
- L2: compressed rendered terminal streams in memory
- L3: compressed search indexes plus page PNG and terminal stream files on disk
- L4: cache miss path that regenerates data from the PDF or PNG

L4 is not a stored cache and has no size setting.

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
search highlight PNGs, and LRU marker files. It does not delete logs.

## Logs

`pdf-tui` prints the active log path to stderr at startup. Startup overwrites
`logs/latest.log` and removes older `*.log` files, so only the latest run is
kept. Logs include PDF page rendering, terminal render requests, cache
hits/misses, preload behavior, and failures from external tools such as
`pdfinfo`, `pdftoppm`, or Chafa.
