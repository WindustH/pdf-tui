# Keymap

Key bindings live in `keymap.toml` next to `config.toml` (by default
`~/.config/pdf-tui/keymap.toml`). The file is created with the defaults on
first run. Actions missing from a section are added back with their default
keys when the file is loaded, so to remove a binding, bind the action to
another key instead of deleting its entry.

## Format

Each section holds a list of bindings:

```toml
[viewer]
keymap = [
  { on = "q", run = "quit", desc = "Quit pdf-tui" },
  { on = ["g", "g"], run = "home", desc = "Go to first page" },
  { on = ["L", "s"], run = "layout scroll 1 3", desc = "Use one-column scroll layout" },
]
```

- `on`: one key, or a list of keys pressed in sequence.
- `run`: the action.
- `desc`: the text shown by `F1` and in which-key hints.

Sections:

| Section | Used in |
| --- | --- |
| `[viewer]` | The page viewer |
| `[metadata]` | The metadata view |
| `[bookmarks]` | The bookmarks view |
| `[search]` | The search view; unbound keys edit the query |
| `[selection]` | The selection view |
| `[input]` | The command prompt and the search query |
| `[global]` | Added to the viewer, metadata, and bookmarks views (default: `:` for the command prompt) |

## Key Names

- Characters: `q`, `G`, `:`, ...; uppercase letters are shifted keys.
- `enter`, `esc`, `space`, `tab`, `backtab` (shift-tab), `backspace`,
  `delete`, `insert`
- `left`, `right`, `up`, `down`, `home`, `end`, `pgup`, `pgdn` (`pageup`,
  `pagedown` also work)
- `f1` ... `f12`
- Modifiers: `ctrl-c`, `alt-x`
- Yazi-style names: `<Enter>`, `<Esc>`, `<Space>`, `<Tab>`, `<S-Tab>`,
  `<PageDown>`, `<C-c>`, `<A-x>`, `<F1>`
- Mouse buttons for the viewer and selection views: `mouse_left`,
  `mouse_right`, `mouse_middle`. `selection_mark` reacts to the press;
  other actions run when the button is released.

## Actions

Viewer:

- `quit`, `help`, `command` (open the prompt)
- `scroll_down`, `scroll_up`: one row (a slice in scroll layouts, a page row
  in grids)
- `page_down`, `page_up`: one screen
- `next_page`, `previous_page`: move the focus by one PDF page
- `home`, `end`
- `refresh`, `clear-cache` (or `clear_cache`)
- `metadata`, `bookmarks`, `search`, `selection`: open a view
- `selection_mark`: place a selection anchor at the mouse position (mouse
  bindings only)
- `selection_cancel`: drop the anchors of a selection in progress
- `layout <name> [args...]`, `layout-use <name> [args...]`: same as the
  [commands](commands.md#layout-name-args)

Metadata:

- `back`, `help`, `quit`, `edit_metadata`
- `metadata_scroll_down`, `metadata_scroll_up`
- `metadata_page_down`, `metadata_page_up`

Bookmarks:

- `back`, `help`, `quit`, `edit_bookmarks`
- `bookmarks_next`, `bookmarks_previous`
- `bookmarks_page_down`, `bookmarks_page_up`
- `bookmarks_toggle`, `bookmarks_toggle_all`
- `bookmarks_open`
- `bookmarks_panel_narrower`, `bookmarks_panel_wider`

Search:

- `back`, `help`, `quit`
- `search_next`, `search_previous`
- `search_page_down`, `search_page_up`
- `search_open`

Selection:

- `back`, `help`, `quit`
- `selection_mark`, `selection_cancel` (cancel anchors, or go back when there
  are none)
- `selection_reselect`: commit a new selection made inside the shown one
- `selection_next`, `selection_previous`
- `selection_copy_text`, `selection_copy_image`

Input (the prompt and the search query):

- `cancel`, `submit`, `help`
- `backspace`, `delete`, `kill_before_cursor`, `kill_after_cursor`
- `move_left`, `move_right`, `move_start`, `move_end`
- `completion_next`, `completion_previous`, `history_previous`,
  `history_next` (command prompt only)
