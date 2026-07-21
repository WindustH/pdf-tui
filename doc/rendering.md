# Rendering

Rendering has two stages:

1. The selected PDF raster backend rasterizes PDF pages into PNG files.
2. `img-tui` or Chafa converts those PNG files into terminal output.

Page PNGs are cached under:

- `~/.cache/pdf-tui/pages/`

Page PNGs are disk-backed. Runtime state keeps only lightweight page metadata
and short-lived decode buffers used while slicing or preparing terminal output.
Temporary backend output is written under the system temp directory, usually
`/tmp/pdf-tui/`, before being copied or renamed into the persistent cache.

`render.pdf_raster_backend` selects `pdfium`, `mutool`, or `poppler`.
`render.pdf_raster_batch_pages` controls how many consecutive pages one raster
batch may render. Batching reduces process startup and PDF reread costs during
sequential reading and preloading.

The Pdfium backend uses a dynamic `libpdfium` library. `pdf-tui` looks at
`render.pdfium_library_path`, then `PDF_TUI_PDFIUM_LIBRARY_PATH`, then packaged
libraries installed next to the executable before falling back to the system
library search path. The Homebrew formula uses this path to bundle Pdfium
without writing user config.

The Mutool backend runs `mutool draw`. `render.mutool_threads`,
`render.mutool_band_height`, and `render.mutool_parallel` control its threaded
banded rendering mode.

Rendered terminal streams are cached under:

- `~/.cache/pdf-tui/render/`

Viewer pages, grid pages, bookmark previews, and search-highlight previews all
go through the same terminal stream render cache after their source PNG exists.

At runtime, already-rendered terminal streams are first kept in raw memory
(L1). Cold protocol streams can be compressed in memory (L2). If both memory
levels miss, `pdf-tui` reads the compressed disk cache (L3). A miss at all
levels regenerates the data (L4).

Embedded-text search indexes are cached under:

- `~/.cache/pdf-tui/text/`

The search cache avoids rerunning `pdftotext -tsv` when the PDF has not changed.
Search-highlight preview PNGs are cached under `~/.cache/pdf-tui/search-highlight/`
and limited by `render.search_highlight_cache_max_bytes`.

## Page And Slice Cache

Grid mode requests whole-page PNGs sized to the target terminal area.

Scroll mode requests page slices. Each slice records metadata such as page
index, slice index, slice count, target dimensions, viewport dimensions, and
scroll divisor. Slice cache keys include those values so incompatible slices
are not reused.

## Terminal Render Cache

Terminal render cache files use zstd compression. Kitty uploads that need a
separate refresh placement are stored as a framed payload containing both the
upload bytes and refresh bytes.

The cache intentionally stores terminal stream data, not transient screen
state. Runtime-only placement state is managed by `img-tui`.

## Preloading

Preloading is tiered by distance from the visible region:

- `render.preload_ahead` and `render.preload_behind` warm the outer page PNG
  cache and trigger batched raster backend output.
- `render.preload_slice_ahead` and `render.preload_slice_behind` warm nearer
  scroll-slice PNGs.
- `render.preload_terminal_ahead` and `render.preload_terminal_behind` warm the
  nearest terminal streams and memory cache entries.

Visible requests use the highest scheduler priority. The page/slice scheduler
orders work as visible requests, then slice preloads, then page PNG preloads.
The terminal-render scheduler orders work as visible requests, then terminal
stream preloads. A queued preload is promoted when it becomes needed by the
visible viewport.

Search preview preloading waits for `render.search_preload_idle_ms` after text
input so filtering does not keep starting work for short-lived result sets.
Moving between search results skips that delay and preloads around the current
selection immediately.

## Protocol Rendering

Native protocol output is not written into Ratatui cells. `img-tui` tracks
protocol overlays and writes image protocol bytes after the Ratatui frame is
flushed. This avoids raw escape sequences appearing as text and prevents blank
intermediate frames when images are replaced.
