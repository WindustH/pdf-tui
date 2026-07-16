# Keymap

Keymaps are stored in:

- `~/.config/pdf-tui/keymap.toml`

The file is split by context:

- `[viewer]`
- `[metadata]`
- `[input]`
- `[global]`

Default entries use compact Yazi-style TOML:

```toml
[viewer]
keymap = [
  { on = "q", run = "quit", desc = "Quit pdf-tui" },
  { on = "f1", run = "help", desc = "Show viewer key bindings" },
  { on = "r", run = "refresh", desc = "Refresh current PDF" },
  { on = "m", run = "metadata", desc = "Show PDF metadata" },
  { on = ["L", "s"], run = "layout scroll 1 3", desc = "Use one-column scroll layout" },
]
```

`on` can be a single key or a key sequence.

Supported key names include:

- characters such as `q`, `h`, `j`, `k`, `l`
- `enter`, `space`, `esc`, `tab`, `backtab`
- `left`, `right`, `up`, `down`
- `home`, `end`, `pgup`, `pgdn`
- Yazi-style names such as `<Enter>`, `<PageDown>`, `<C-c>`

## Actions

Viewer actions:

- `quit`
- `command`
- `help`
- `scroll_down`, `scroll_up`
- `page_down`, `page_up`
- `next_page`, `previous_page`
- `home`, `end`
- `clear-cache`, `clear_cache`
- `refresh`
- `metadata`
- `layout <name> [args...]`
- `layout-use <name> [args...]`

Layout actions use the same syntax as `:layout` or `:layout-use`, without the
leading `:`.

Metadata actions:

- `back`
- `help`
- `edit_metadata`
- `metadata_scroll_down`, `metadata_scroll_up`
- `metadata_page_down`, `metadata_page_up`

Input actions:

- `cancel`, `submit`
- `help`
- `backspace`, `delete`
- `move_left`, `move_right`, `move_start`, `move_end`
- `kill_before_cursor`, `kill_after_cursor`
- `completion_next`, `completion_previous`
- `history_previous`, `history_next`

Input actions are interpreted by `framework-tui`, so prompt behavior stays
consistent with `gallery-tui`.
