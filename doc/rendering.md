# Rendering

Rendering has two stages:

1. `pdftoppm` rasterizes PDF pages into PNG files.
2. `img-tui` or Chafa converts those PNG files into terminal output.

Page PNGs are cached under:

- `~/.cache/pdf-tui/pages/`

Page PNGs are disk-backed. Runtime state keeps only lightweight page metadata
and short-lived decode buffers used while slicing or preparing terminal output.
Temporary `pdftoppm` output is written under the system temp directory, usually
`/tmp/pdf-tui/`, before being copied or renamed into the persistent cache.

`render.pdftoppm_batch_pages` controls how many consecutive pages one
`pdftoppm` process may render. Batching reduces process startup and PDF reread
costs during sequential reading and preloading.

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

`render.preload_ahead` and `render.preload_behind` define the preload window
around the visible region.

Preloads share the same global concurrency limit as visible requests, but they
use a separate permit path that leaves capacity for visible work when possible.

## Protocol Rendering

Native protocol output is not written into Ratatui cells. `img-tui` tracks
protocol overlays and writes image protocol bytes after the Ratatui frame is
flushed. This avoids raw escape sequences appearing as text and prevents blank
intermediate frames when images are replaced.
