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
