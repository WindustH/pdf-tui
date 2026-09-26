# Troubleshooting

The path of the run log is printed when `pdf-tui` starts; failures of
external tools and renders are recorded there in detail.

## Submodules Are Missing

If Cargo reports missing `framework-tui` or `img-tui` while building from
source:

```sh
git submodule update --init --recursive
```

## The Document Does Not Open

`pdfinfo` from Poppler must be installed: it reads the page count and page
sizes. Documents without pages cannot be opened.

## Pages Stay Blank Or Show "render failed"

- With the default Pdfium backend, check that the Pdfium library is found
  (see [Rendering](rendering.md#raster-backends)), or switch backends:

  ```toml
  [render]
  pdf_raster_backend = "poppler"
  ```

- If only terminal rendering fails, try the text fallback:

  ```sh
  GALLERY_TUI_RENDER_MODES=symbols pdf-tui file.pdf
  ```

## Raw Escape Sequences Appear

The terminal or multiplexer does not support the detected graphics protocol.
Force another mode order:

```sh
GALLERY_TUI_RENDER_MODES=sixel,symbols pdf-tui file.pdf
GALLERY_TUI_RENDER_MODES=symbols,ascii pdf-tui file.pdf
```

or turn detection off, which uses only Chafa symbols and ASCII:

```toml
[render]
auto_detect = false
```

## Rendering Is Slow

Pages are rasterized at the pixel size of their terminal area, so large
windows and terminals with large cells produce large images. To reduce the
work:

- Use fewer concurrent jobs or a shorter preload window when the machine is
  busy:

  ```toml
  [render]
  max_concurrent = 2
  preload_ahead = 2
  preload_behind = 1
  ```

- Try another raster backend (`pdfium` is usually the fastest).
- Turn off `behavior.frame_sync_navigation_viewer` if navigation feels
  blocked while pages render.

## Cache Uses Too Much Space

Lower the limits (bytes):

```toml
[render]
cache_max_bytes = 268435456
raw_memory_cache_max_bytes = 16777216
compressed_memory_cache_max_bytes = 67108864
prepared_memory_cache_max_bytes = 67108864
```

or run `:clear-cache`. The disk limit is applied at the next start.

## Metadata, Bookmarks, Search, Or Copying Are Unavailable

These features need external tools: `exiftool` (metadata), `pdftk`
(bookmarks), `pdftotext` (search and copying selected text), and `wl-copy`,
`xclip`, or `xsel` on Linux (copying). The view shows the error when a tool is
missing. Search only finds embedded text; scanned pages need OCR first.

## The Editor Does Not Open

Metadata and bookmark editing run `$EDITOR`, then `$VISUAL`, then `vi`
(`notepad` on Windows). On Unix the command runs through `sh`, so it may
include arguments (for example `EDITOR="code --wait"`); the editor must not
return before the file is saved.
