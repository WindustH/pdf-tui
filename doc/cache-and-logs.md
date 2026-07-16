# Cache And Logs

Cache and logs are stored under:

- `~/.cache/pdf-tui/`

Important subdirectories:

- `pages/`: PDF page and slice PNG cache
- `render/`: compressed terminal render stream cache
- `pdf-tui.log`: default log file

## Cache Cleanup

The render cache is limited by:

```toml
[render]
cache_max_bytes = 536870912
```

When the cache exceeds the limit, older entries are removed using LRU marker
files.

Clear cache from inside the TUI:

```text
:clear-cache
```

This clears cached page PNGs, rendered terminal streams, and LRU marker files.
It does not delete logs.

## Logs

`pdf-tui` prints the active log path to stderr at startup. Logs include PDF
page rendering, terminal render requests, cache hits/misses, preload behavior,
and failures from external tools such as `pdfinfo`, `pdftoppm`, or Chafa.
