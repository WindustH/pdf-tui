# Cache And Logs

## Location

The cache directory is `$XDG_CACHE_HOME/pdf-tui`, or `$HOME/.cache/pdf-tui`
when `XDG_CACHE_HOME` is unset (also on macOS); on Windows without either
variable it is `%LOCALAPPDATA%\pdf-tui`.

| Path | Contents |
| --- | --- |
| `cache.sqlite3` | Page/slice PNGs, slice metadata, terminal renders, search indexes and selection/highlight images |
| `cache.sqlite3-wal`, `cache.sqlite3-shm` | SQLite WAL and coordination files |
| `logs/` | Run logs |
| `runtime/` | One lock file per running instance |
| `editor/`, `bookmarks/` | Temporary files while editing metadata or bookmarks |

Backends still use temporary work files. File-only image consumers get private,
content-addressed files in the system temporary directory from `img-tui`;
these are removed when the cache session closes normally.

On the first start with SQLite caching, the old `pages/`, `render/`, `text/`,
`search-highlight/` and `selection/` payload directories are deleted. Old
payloads are not imported or eagerly regenerated. New entries are computed
only when requested. Render job lock files may still use those directories.

## Reading Positions

Remembered reading positions (`behavior.remember_reading_position`) are not
cache data, so they live in `progress.toml` in the state directory:
`$XDG_STATE_HOME/pdf-tui`, or `$HOME/.local/state/pdf-tui` when
`XDG_STATE_HOME` is unset (also on macOS); on Windows without either variable
it is `%APPDATA%\pdf-tui`. Deleting the cache directory keeps them. Older
versions kept `progress.toml` in the cache directory; it is moved to the state
directory on the next start.

## Limits And Cleanup

Persistent caching is implemented by `img-tui`, using SQLite BLOBs and
transactional least-recently-used pruning. Last-use updates are coarsened to
one minute. `render.disk_cache_max_bytes` (512 MiB, `0` means unlimited) is enforced
on writes and at startup. Search highlights and selections also retain their
individual 64 MiB budgets. A newly written oversized entry can exceed the
global budget by itself so it can still be displayed.

These limits count live payload bytes. SQLite pages, free space, WAL files,
and temporary images held for active consumers are additional storage.
Explicit cleanup performs a passive WAL checkpoint and incremental vacuum;
deleting rows does not promise immediate reduction of the physical DB file.

Memory caches use moka with an explicit LRU policy and byte weights:
`render.raw_memory_cache_max_bytes` (32 MiB),
`render.compressed_memory_cache_max_bytes` (128 MiB, when
`render.memory_compression` is enabled), and
`render.prepared_memory_cache_max_bytes` (128 MiB). Protocol results are stored
in the raw and compressed tiers independently. Limits are approximate during
concurrent maintenance; one oversized result and a currently borrowed render
may remain alive beyond the budget.

`:clear-cache` clears SQLite payloads and this instance's memory caches.
Logs, configuration and reading positions are kept. SQLite transactions make
this safe while other instances run: they retain their own active image files.
Background jobs and new requests may populate fresh entries after a clear.

## Several Instances

Each application uses its own `cache.sqlite3`. Instances of pdf-tui can share
one local database through WAL, short transactions and a busy timeout.
Per-entry OS locks still serialize expensive PDF rendering. Cache clearing
and trimming no longer depend on the number of running instances. Entry `.lock`
files remain in place after use so waiting processes keep locking the same file;
the operating system releases ownership when the holder exits. Do not manually
delete lock files while an instance is running.

## Logs

Each run writes `logs/run-<pid>-<time>.log`; the path is printed when
`pdf-tui` starts. `logs/latest.log` points to the newest run (a symlink on
Unix, a file containing the path on Windows). When no other instance is
running, older logs are deleted at startup.

Logs are written at debug level: page and slice renders, terminal renders,
cache use, preloading, and failures of external tools such as `pdfinfo`,
the raster backend, or Chafa. Attach the log when reporting a problem.
