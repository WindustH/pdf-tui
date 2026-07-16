# Keymap

Keymaps are stored in:

- `~/.config/pdf-tui/keymap.toml`

The file is split by context:

- `[browser]`
- `[detail]`
- `[input]`
- `[global]`

Default entries use compact Yazi-style TOML:

```toml
[browser]
keymap = [
  { on = "q", run = "quit", desc = "Quit pdf-tui" },
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

Browser actions:

- `quit`
- `command`
- `scroll_down`, `scroll_up`
- `page_down`, `page_up`
- `next_page`, `previous_page`
- `home`, `end`
- `clear-cache`, `clear_cache`
- `layout <name> [args...]`
- `layout-use <name> [args...]`

Layout actions use the same syntax as `:layout` or `:layout-use`, without the
leading `:`.

Input actions:

- `cancel`, `submit`
- `backspace`, `delete`
- `move_left`, `move_right`, `move_start`, `move_end`
- `kill_before_cursor`, `kill_after_cursor`
- `completion_next`, `completion_previous`
- `history_previous`, `history_next`

Input actions are interpreted by `framework-tui`, so prompt behavior stays
consistent with `gallery-tui`.
