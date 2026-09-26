# Views

## Scroll Layout

```text
scroll <columns> <scroll_divisor>
```

Pages are shown in `columns` columns and cut into horizontal slices of at most
1/`scroll_divisor` of the screen height; one scroll step (`j`/`k`) moves one
slice, and `h`/`l` move a screen. The page width is chosen to cover as much of
the screen as possible across all scroll positions. Pages side by side are
sliced on the same grid, so their slices line up.

Each slice is its own image, cached, preloaded, and drawn independently. This
simulates continuous scrolling without the flicker and transitional frames of
cropping one large terminal image.

## Grid Layout

```text
grid <rows> <columns>
```

Whole pages in a fixed grid. `j`/`k` move by one grid row, `h`/`l` by a whole
grid. The status line shows the range of visible pages.

## Reading Progress

Positions are measured in pages from 0: `0.0` is the start of the first page,
`2.5` the middle of the third. The position of a view is the average of the
visible page regions, weighted by their visible area. When the layout or the
terminal size changes, `pdf-tui` picks the scroll position of the new layout
whose value is closest, so the same part of the document stays in view.
`--progress` and remembered positions are applied the same way; a whole
number such as `4` lands on the page that starts there.

## Frame-Synced Navigation

With frame sync on for a view (by default only the viewer), browsing actions
are ignored until every image of the current frame has been drawn, so key
repeat cannot skip over pages that were never shown. See the
`frame_sync_navigation_*` settings in [Configuration](configuration.md#behavior).

## Metadata

`m` shows the file name, path, page count, page sizes, and the tags reported
by `exiftool`. `e` opens these editable fields in `$EDITOR` as TOML: `Title`,
`Author`, `Subject`, `Keywords`, `Creator`, `Producer`, `CreationDate`, and
`ModifyDate`. An empty string clears a field. After saving and closing the
editor, a dialog lists the changes; `y` writes them with `exiftool` and
reloads the document.

## Bookmarks

`b` shows the PDF outline (read with `pdftk`) as a tree, with a preview of the
selected bookmark's page on the right. On entry the bookmark closest to the
reading position is selected and its parents are expanded; other entries start
collapsed, and expansion is kept for the session.

`space` expands or collapses the selected entry, `z` expands everything or
collapses everything, and `enter` shows the bookmarked page from its top.
`h`/`l` resize the tree panel; the initial ratio is set by
`behavior.bookmarks_left_ratio` and `bookmarks_right_ratio`.

`e` opens the outline in `$EDITOR` as `[[bookmark]]` tables with `level`
(1 for top level), `page` (1-based), and `title`. Delete a table to remove a
bookmark or add one to create it. After confirmation (`y`), `pdftk` writes a
new copy of the PDF, which replaces the original with its file permissions
kept.

## Search

`s` opens a query box with the result list below it and a preview of the
selected result's page on the right. The first search builds an index of the
embedded text with `pdftotext -tsv` (cached until the PDF changes); `pdf-tui`
does no OCR, so scanned pages without a text layer have no matches.

Matching ignores ASCII case and all whitespace, so `o w` finds `Hello World`:
a query can span several words of one text line, but not a line break. At
most 2000 results are listed. Each shows its line with the match highlighted, and the
preview inverts the matched area on the page.

`enter` jumps to the result, centering the match where the layout allows, and
keeps it inverted on the page until the next navigation in the viewer.

Preview preloading waits for a pause in typing
(`render.search_preload_idle_ms`) so a changing query does not start work for
results that are about to disappear.

## Selection

Selections are rectangles in page coordinates, marked with the mouse:

1. Press the left button on a page. A small crosshair marks the anchor under
   the pointer.
2. Either drag and release elsewhere, or click again: this places the opposite
   corner. The rectangle is drawn as an inverted outline and added to the
   selection history.
3. Further clicks move the nearer corner. A click outside the page is clamped
   to the page edge. `esc` removes the anchors (and an unfinished selection).

Only the terminal cell under the pointer is known, so an anchor is the center
of that cell.

`v` opens the selection view, which shows the current selection as large as
the terminal allows. `j`/`k` or the mouse wheel move through the history. `y`
copies the embedded text inside the rectangle (built from the same index as
search) and `Y` copies a PNG of it, rendered anew from the PDF at up to
`render.selection_image_max_pixels` pixels. Copying uses `wl-copy`, `xclip`,
or `xsel` on Linux and `pbcopy` or `osascript` on macOS; Windows has no
clipboard support yet.

Selecting inside the selection view creates a smaller selection within the
shown one; it is inserted after its parent in the history. `v` commits it and
shows it; moving to another selection commits it as well.

The history lasts for the session and is cleared when the document is
reloaded.
