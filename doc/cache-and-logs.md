# Cache And Logs

## Location

The cache directory is `$XDG_CACHE_HOME/pdf-tui`, or `$HOME/.cache/pdf-tui`
when `XDG_CACHE_HOME` is unset (also on macOS); on Windows without either
variable it is `%LOCALAPPDATA%\pdf-tui`.

| Path | Contents |
| --- | --- |
| `pages/` | Page and slice PNGs; `.toml` files describe slices |
| `render/` | Terminal output, zstd-compressed (`*.ansi`) |
| `text/` | Search indexes (`*.toml.zst`) |
| `search-highlight/` | Pages with a search match inverted |
| `selection/` | Selection crops and pages with selection marks |
| `progress.toml` | Remembered reading positions (`behavior.remember_reading_position`) |
| `logs/` | Run logs |
| `runtime/` | One lock file per running instance |
| `editor/`, `bookmarks/` | Temporary files while editing metadata or bookmarks |

Backends write their output to `pdf-tui/pages/` in the system temporary
directory first (usually `/tmp`) and move finished files into the cache.

## Limits And Cleanup

Each cache entry has a `.used` marker whose modification time records when
the entry was last used (refreshed at most once a minute). Cleanup removes the
least recently used entries first:

- At startup the whole cache is trimmed to `render.cache_max_bytes` (512 MiB
  by default; `0` disables it). Leftover markers, slice descriptions, and
  temporary files of entries that no longer exist are removed as well.
- `search-highlight/` and `selection/` are trimmed to
  `render.search_highlight_cache_max_bytes` and
  `render.selection_cache_max_bytes` (64 MiB each) whenever an image is added.

Page and terminal caches can therefore grow past the limit during a long
session; the next start trims them.

In memory, terminal output is kept up to `render.raw_memory_cache_max_bytes`
(32 MiB); older protocol output is compressed and kept up to
`render.compressed_memory_cache_max_bytes` (128 MiB) when
`render.memory_compression` is on. Decoded images for protocol rendering use
up to `render.prepared_memory_cache_max_bytes` (128 MiB).

`:clear-cache` deletes `pages/`, `render/`, `text/`, `search-highlight/`, and
`selection/`. Logs and reading positions are kept.

## Several Instances

Instances can share one cache directory. Every entry is written to a
temporary file next to it and then moved into place atomically (`rename` on
Unix, `MoveFileExW` on Windows), so readers never see partial files. Work on
the same entry is serialized with lock files held by an OS file lock (`flock`
on Unix, `LockFileEx` on Windows); the lock of a process that died is
reclaimed automatically.

While another instance runs, startup cleanup and log removal are skipped and
`:clear-cache` is refused, so no instance deletes files another one is
showing.

## Logs

Each run writes `logs/run-<pid>-<time>.log`; the path is printed when
`pdf-tui` starts. `logs/latest.log` points to the newest run (a symlink on
Unix, a file containing the path on Windows). When no other instance is
running, older logs are deleted at startup.

Logs are written at debug level: page and slice renders, terminal renders,
cache use, preloading, and failures of external tools such as `pdfinfo`,
the raster backend, or Chafa. Attach the log when reporting a problem.
