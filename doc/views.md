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
