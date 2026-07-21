# Views

`pdf-tui` has two reading layouts: scroll and grid.

## Scroll

Scroll mode is a static slice-based simulation of continuous scrolling.

Syntax:

```text
scroll <columns> <scroll_divisor>
```

`columns` controls how many page columns are shown. `scroll_divisor` controls
the maximum height of each page slice relative to the available display area.
For example, a divisor of `3` means one movement step is approximately one
third of the usable height.

Pages are split into horizontal slices. Slices from the same page are adjacent
without gaps. Each slice is handled like an independent image for caching,
preloading, and terminal drawing.

This avoids the fragile transitional frames that true terminal-image cropping
can produce while still preserving a reading progress model close to
continuous scrolling.

## Grid

Grid mode shows whole pages in a fixed grid.

Syntax:

```text
grid <rows> <columns>
```

Navigation moves by rows, not by per-page focus. This matches PDF reading:
the view is a window over pages rather than an image gallery with a focused
item.

## Progress

Reading progress is 0-based. The start of the first page is `0.0`.

The progress model combines visible page regions into a weighted value. When
switching layouts, `pdf-tui` finds the closest legal state in the new layout
so the visible reading position remains stable.

## Frame-Synced Navigation

When the current view's frame-sync switch is enabled, image-browsing actions
wait for the current frame to finish rendering before accepting another browse
action. Viewer sync is enabled by default; bookmark and search preview sync are
disabled by default. Command input, editing, help, refresh, and non-image
metadata navigation remain available.

## Metadata

The metadata view shows PDF file information and PDF metadata reported by
`exiftool`.

Editable fields are written through `exiftool` after an explicit confirmation.
The default viewer key is `m`; the default metadata edit key is `e`.

## Bookmarks

The bookmarks view shows the PDF outline reported by `pdftk`.

The view has two panels:

- the left panel is a collapsible bookmark tree
- the right panel previews the page targeted by the hovered bookmark

The default panel ratio is `2:1` and can be adjusted in the running session
with the bookmark panel width actions. On first entry, bookmark children are
collapsed. When entering the view, `pdf-tui` selects the bookmark closest to the
current 0-based reading progress and expands only the necessary parent entries.
Expansion state is retained for the rest of the session.

`space` expands or collapses the hovered entry. `z` expands every entry on the
first press and collapses every entry on the next press. `enter` jumps to the
hovered bookmark progress. The default viewer key is `b`; the default bookmark
edit key is `e`.

Editable bookmarks are written through `pdftk` after an explicit confirmation.

## Search

The search view finds text embedded in the PDF. It does not run OCR.

The view has two panels:

- the left panel contains a search box and live result list
- the right panel previews the page for the selected result

The default panel ratio is `2:1`. As text is typed in the search box, results
are recomputed from an in-memory index built with `pdftotext -tsv`. Each result
shows a one-line context with the matched words highlighted. The preview uses a
temporary highlighted PNG, so the inverted match rectangle is visible with
native terminal image protocols as well as Chafa fallback rendering.

## Selection

The viewer can create rectangular selections with the configured
`selection_mark` action. By default this is `mouse_left`.

The first click must be on a visible PDF page. `pdf-tui` inverts the terminal
cell under the pointer with a small centered crosshair marker. Mouse drags do
not update the selection continuously; the mark is processed on mouse release.
The anchor position is stored as the page coordinate under the mouse event; the
terminal cell size only controls the marker size. Terminal mouse input is
cell-based, so this coordinate is the center of the reported terminal cell.
Press `esc` to cancel active anchors.

The second click places the opposite anchor and creates a selection draft, but
does not immediately switch views. Once both anchors exist, the rectangle
defined by their page-coordinate points is drawn with an inverted outline.
Later clicks move whichever anchor is closer to the new pointer position. If
the pointer is outside the anchor page, the endpoint is derived from the
intersection between the page region and the rectangle formed with the fixed
opposite anchor.

Finished selections are kept as session history. The viewer key `v` opens that
history. The selection view centers the selected region and displays it as
large as the available terminal area allows. In the selection view, the same
`selection_mark` action can create a new selection inside the currently shown
selection; that child selection is inserted directly after its parent in the
session history. Press `v` in the selection view to commit and focus a child
selection draft, or to prompt for a new child selection when no draft exists.
Committed child selections are rendered again from the PDF source rather than
cropped from the parent preview. Browsing to another selection also commits the
draft before moving.

`j`/`k`, arrows, page keys, or mouse wheel browse the selection history. `y`
copies embedded text inside the selection through the cached text index. `Y`
renders and copies a PNG of the selection.
