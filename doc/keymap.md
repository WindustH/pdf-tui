# Keymap

Keymaps are stored in:

- `~/.config/pdf-tui/keymap.toml`

The file is split by context:

- `[viewer]`
- `[metadata]`
- `[bookmarks]`
- `[search]`
- `[selection]`
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
  { on = "b", run = "bookmarks", desc = "Show PDF bookmarks" },
  { on = "s", run = "search", desc = "Search PDF text" },
  { on = "v", run = "selection", desc = "Show PDF selections" },
  { on = "mouse_left", run = "selection_mark", desc = "Mark PDF selection corner" },
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
- mouse tokens such as `mouse_left`

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
- `bookmarks`
- `search`
- `selection`
- `selection_mark`
- `selection_cancel`
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

Bookmark actions:

- `back`
- `help`
- `edit_bookmarks`
- `bookmarks_next`, `bookmarks_previous`
- `bookmarks_page_down`, `bookmarks_page_up`
- `bookmarks_toggle`
- `bookmarks_toggle_all`
- `bookmarks_open`
- `bookmarks_panel_narrower`, `bookmarks_panel_wider`

Search actions:

- `back`
- `help`
- `search_next`, `search_previous`
- `search_page_down`, `search_page_up`
- `search_open`

Selection actions:

- `back`
- `help`
- `selection_mark`
- `selection_cancel`
- `selection_reselect`
- `selection_next`, `selection_previous`
- `selection_copy_text`
- `selection_copy_image`

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
