# Rendering

Rendering has two stages:

1. `pdftoppm` rasterizes PDF pages or page slices into PNG files.
2. `img-tui` or Chafa converts those PNG files into terminal output.

Page PNGs are cached under:

- `~/.cache/pdf-tui/pages/`

Rendered terminal streams are cached under:

- `~/.cache/pdf-tui/render/`

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
