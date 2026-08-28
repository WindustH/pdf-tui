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

For [Zellij 0.45 and newer](https://zellij.dev/documentation/compatibility.html),
`img-tui` actively queries KGP support and selects Kitty when both Zellij and
the attached host terminal confirm it. Outer-terminal environment variables
are not treated as proof, so an unsupported host or
`support_kitty_graphics_protocol false` still falls back safely. Zellij does
not currently support Kitty Unicode placeholders, so pdf-tui uses regular
Kitty placements in this environment.

Sixel remains disabled by default under Zellij unless
`render.zellij_sixel` is set to `auto` or `on`. This option controls Sixel only
and does not disable the automatically detected Kitty path.

## Kitty Placeholders

`pdf-tui` uses `img-tui` terminal capability detection for Kitty unicode
placeholder support. This keeps placement and erase behavior aligned with
`gallery-tui`.
