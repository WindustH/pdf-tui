# Architecture

`pdf-tui` is a single binary crate built on two libraries, included as git
submodules:

- `crates/framework-tui`: key dispatch with multi-key sequences, prompt
  editing, command history and completion, footer and popup widgets, and the
  `$EDITOR` helper
- `crates/img-tui`: terminal capability detection, native image protocols
  (Kitty, Sixel, iTerm2), and protocol overlay management across frames

Initialize them with `git submodule update --init --recursive`.

## Data Flow

One tokio task owns all state and runs the event loop (`event_loop.rs`). It
draws a frame, waits for the next `AsyncEvent`, applies it and any events
already queued, and redraws if something visible changed.

Events come from:

- the terminal input thread and the optional file watcher (`background.rs`)
- background jobs: page rasterizing, marking, terminal rendering, reloads,
  PDF edits, search indexing, selection crops, clipboard copies, and cache
  clearing

Drawing requests what it needs: a missing page PNG or terminal render is
queued as a job and drawn when its event arrives. Every job result carries
enough identity (document size and modification time, or a cache key) to be
dropped when it no longer matches the current state.

```text
PDF --(pdf/ PageStore)--> page/slice PNG --(overlay/ OverlayStore)--> marked PNG
    --(render/ RenderStore)--> terminal output --(ui/, img-tui)--> screen
```

## Modules

Startup and runtime:

- `main.rs`: command line, startup (config, cache, logging, terminal
  detection, opening the document), and saving the reading position on exit
- `event_loop.rs`: `Session`, the main loop, the external editor hand-off,
  and one handler per `AsyncEvent`
- `event.rs`: `AsyncEvent` and the job outcome types
- `background.rs`: the input thread (paused while an editor owns the
  terminal) and the auto-refresh file watcher
- `terminal.rs`: terminal setup and restore, buffered output, and the panic
  hook
- `logging.rs`: per-run log files

Application state (`app/`), with `App` extended by one module per concern:

- `mod.rs`: `App`, view modes, dialogs, key help, and page/slice results
- `input.rs`: routing keys and mouse events to actions; redraw detection
- `commands.rs`: the command prompt and `:layout`
- `tasks.rs`: starting background jobs (reloads, PDF edits, cache clearing,
  search index, selection crops, clipboard)
- `navigation.rs`: scrolling, page jumps, and the cached scroll layout
- `progress.rs`: mapping between scroll positions and reading progress
- `metadata_state.rs`, `bookmark_state.rs` (`BookmarkTree`),
  `search_state.rs` (`SearchState`), `search_jump.rs`: the metadata,
  bookmark, and search views
- `selection_state.rs` (`SelectionState`), `selection_hit.rs`,
  `selection_geometry.rs`: selections, mapping mouse positions to page
  coordinates, and the underlying geometry

Geometry:

- `layout.rs`: the scroll layout (page slices, rows, visible rows, placement
  on screen) and grid slots
- `geometry.rs`: fitting pages into cells and splitting panels, shared by
  drawing, preloading, and hit testing

Image pipeline:

- `job_queue.rs`: the priority scheduler shared by the page and render stores
  (visible work first, one slot reserved for it, promotion of preloads)
- `pdf/`: `PdfDocument` (page count and sizes from `pdfinfo`), `PageStore`
  (scheduling), and `raster/` (backends, page batches, slices, file cache)
- `overlay.rs`: `OverlayStore`, producing search-highlighted and
  selection-marked copies of page images off the UI thread
- `render/`: `RenderStore`, rendering with mode fallbacks, the Chafa driver,
  cache keys, the on-disk render cache format, and the in-memory caches
- `ui/`: frame composition (`ui.rs`), page drawing primitives (`page.rs`),
  one module per view, the footer and modals, and preloading around the view

Document features:

- `search/`: building and caching the `pdftotext` index, matching, and
  highlight images
- `selection/`: selection types, marks drawn into images, and crop rendering
- `metadata.rs`: reading and writing metadata with `exiftool`
- `bookmarks.rs`: reading and writing the outline with `pdftk`
- `clipboard.rs`: copying text and PNGs with the platform's clipboard tools
- `progress_store.rs`: remembered reading positions

Configuration and cache:

- `config/`: loading and normalizing `config.toml`, `keymap.toml`, and
  `theme.toml`
- `cache/`: locks shared between instances (`lock.rs`), atomic writes and
  LRU markers (`files.rs`), and size limits and clearing (`cleanup.rs`)

## Refresh

Manual `:refresh`, the viewer `r` key, and automatic refresh share one path:
a background job reopens the PDF and rereads metadata and bookmarks, then the
event loop replaces the document, clears page, overlay, render, search, and
selection state, and re-applies the reading progress. Results of jobs started
for the old file are ignored because their document identity no longer
matches.

## Editing

Metadata and bookmark editing pause the input thread, suspend the TUI, and
run `$EDITOR` through `framework-tui`. After the editor exits, the terminal is
restored, stray input is discarded, and the edit is shown for confirmation
before a background job writes it.
