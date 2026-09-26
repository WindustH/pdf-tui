# Rendering

A page goes through three stages, each with its own background jobs and
cache:

1. **Rasterizing**: the raster backend renders the page to a PNG at the pixel
   size of its terminal area (cell count times cell size). Scroll layouts
   then cut the page PNG into slice PNGs.
2. **Marking** (only when needed): a copy of the PNG gets the search
   highlight or selection marks drawn in.
3. **Terminal rendering**: the PNG becomes terminal output, either escape
   sequences of a graphics protocol or Chafa text. See
   [Terminal Graphics](terminal-graphics.md).

While a stage runs, the status line shows what is being rendered. With a
graphics protocol the previous images stay on screen meanwhile.

## Raster Backends

`render.pdf_raster_backend` selects the backend:

- `pdfium` (default): renders in-process through the Pdfium library. It is
  loaded from `render.pdfium_library_path`, then `PDF_TUI_PDFIUM_LIBRARY_PATH`,
  then `pdfium/lib/` next to the executable or one directory up, then the
  system library path. A path may name the library file or its directory.
  The Homebrew formula uses the environment variable to bundle Pdfium.
- `poppler`: runs `pdftoppm`.
- `mutool`: runs `mutool draw` with the `mutool_*` settings.

`render.pdf_raster_batch_pages` consecutive pages are rendered together, which
saves process starts and PDF parsing while reading sequentially.

`pdfinfo` is always needed: it provides the page count and page sizes before
anything is drawn.

## Scheduling And Preloading

Rasterizing and terminal rendering each run at most `render.max_concurrent`
jobs at a time. Work for what is on screen goes first; preloads never take
the last slot. A queued preload is promoted when its page becomes visible, and
search previews queued for an outdated query are dropped.

Preloading is tiered by distance from the view:

- `render.preload_ahead`/`preload_behind`: page PNGs (outer ring)
- `render.preload_slice_ahead`/`preload_slice_behind`: scroll slice PNGs
- `render.preload_terminal_ahead`/`preload_terminal_behind`: terminal output
  (nearest ring)

In scroll layouts the distances count scroll rows, in grids pages, and in the
bookmark, search, and selection views list entries.

## Caches

Results are cached on disk, so revisited pages appear quickly, also in later
sessions:

- `pages/`: page and slice PNGs, plus a `.toml` description of each slice
- `render/`: terminal output, zstd-compressed (`*.ansi`)
- `text/`: search indexes
- `search-highlight/` and `selection/`: marked copies and selection crops

Cache entries are keyed by the file's path, size, and modification time, the
pixel size, the backend, and the relevant settings, so a changed PDF or
setting never reuses stale images. Terminal output is also kept in memory:
recent renders as they are (`raw_memory_cache_max_bytes`), older protocol
renders compressed (`compressed_memory_cache_max_bytes`).

Details on sizes, cleanup, and sharing between instances are in
[Cache And Logs](cache-and-logs.md).

## Selections

The selection view and `Y` render just the selected region: Poppler with
`pdftoppm` crop options and Pdfium into a crop-sized bitmap. Mutool has no
reliable crop option, so a temporary full page is rendered and cropped.
Previews are rendered at the size the terminal area needs;
`render.selection_image_max_pixels` limits only the copied PNG.
