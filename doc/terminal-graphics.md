# Terminal Graphics

`pdf-tui` draws pages with the best method the terminal supports, trying them
in this order:

1. Kitty graphics protocol
2. Sixel
3. iTerm2 inline images
4. Chafa symbols (colored block characters)
5. ASCII (Chafa without color)

With `render.auto_detect = true` (the default), `pdf-tui` probes the terminal
at startup and uses the protocols it confirms. With `auto_detect = false`,
only Chafa symbols and ASCII are used. If a method fails for a page, the next
one is tried.

## Choosing Modes

The `GALLERY_TUI_RENDER_MODES` environment variable overrides the detected
order with a comma-separated list:

```sh
GALLERY_TUI_RENDER_MODES=kitty,sixel,symbols pdf-tui file.pdf
GALLERY_TUI_RENDER_MODES=symbols pdf-tui file.pdf
```

Accepted names are `kitty`, `sixel`, `iterm` (or `iterm2`), `symbols`, and
`ascii`; `off` means `symbols,ascii`, and `auto` keeps detection. The variable
is shared with `gallery-tui` because both use the `img-tui` library.

## Multiplexers

Inside tmux and GNU screen, protocol output is wrapped for passthrough
automatically.

Under [Zellij 0.45 and newer](https://zellij.dev/documentation/compatibility.html),
Kitty graphics are used only when Zellij and the host terminal both confirm
support when queried; environment variables of the outer terminal are not
trusted, so an unsupported host, or `support_kitty_graphics_protocol false`,
falls back safely. Zellij does not support Kitty Unicode placeholders, so
regular Kitty placements are used there.

Sixel stays off under Zellij unless `render.zellij_sixel` is `auto` (use it
when the probe confirms Sixel) or `on`. This setting does not affect Kitty.

## Kitty Placeholders

Where the terminal supports Kitty Unicode placeholders, images are placed
through placeholder characters in the text grid, so dialogs and popups
cleanly cover parts of a page image.
