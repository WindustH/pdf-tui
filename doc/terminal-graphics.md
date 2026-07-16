# Terminal Graphics

`pdf-tui` uses `img-tui` to detect terminal graphics support and choose a
render mode order.

Native image protocols:

- Kitty graphics protocol
- Sixel
- iTerm2 inline images

Fallback modes:

- Chafa symbols
- ASCII symbols

## Override Render Modes

The render mode order can be overridden with the shared `img-tui` environment
variable:

```sh
GALLERY_TUI_RENDER_MODES=kitty,sixel,symbols pdf-tui file.pdf
GALLERY_TUI_RENDER_MODES=symbols pdf-tui file.pdf
```

The environment variable name is shared with `gallery-tui` because it comes
from `img-tui`.

## Multiplexers

`img-tui` detects tmux and screen passthrough and configures protocol wrapping
for Chafa and native image protocols.

For zellij, Sixel is disabled by default unless `render.zellij_sixel` is set
to `auto` or `on`.

## Kitty Placeholders

`pdf-tui` uses `img-tui` terminal capability detection for Kitty unicode
placeholder support. This keeps placement and erase behavior aligned with
`gallery-tui`.
