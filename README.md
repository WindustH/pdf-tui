# pdf-tui

`pdf-tui` is a small terminal PDF reader built on `framework-tui` and
`img-tui`.

```sh
pdf-tui /path/to/file.pdf
pdf-tui --progress 0.0 /path/to/file.pdf
pdf-tui /path/to/file.pdf scroll 1 3
pdf-tui /path/to/file.pdf scroll 2 3
pdf-tui /path/to/file.pdf grid 2 3
```

Runtime commands are available with `:`:

```text
layout scroll <columns> <scroll_divisor>
layout grid <rows> <columns>
clear-cache
quit
```

The scroll layout renders pages at the full available column width and crops the
visible slice while scrolling. It does not shrink a full page to fit the
viewport height.

The current renderer uses `pdfinfo` and `pdftoppm` from Poppler to rasterize PDF
pages lazily into the cache directory before handing the resulting images to
`img-tui`.
