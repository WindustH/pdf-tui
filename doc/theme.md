# Theme

Colors are set in `theme.toml` next to `config.toml` (by default
`~/.config/pdf-tui/theme.toml`).

A color is one of:

- a name: `reset` (the terminal default), `black`, `red`, `green`, `yellow`,
  `blue`, `magenta`, `cyan`, `gray`, `dark_gray`, `light_red`, `light_green`,
  `light_yellow`, `light_blue`, `light_magenta`, `light_cyan`, `white`
- `ansi:<0-255>` for a color of the 256-color palette
- `#rrggbb`

Unknown values fall back to `reset`.

| Field | Default | Used for |
| --- | --- | --- |
| `foreground` | `white` | Text |
| `background` | `reset` | Background |
| `muted` | `dark_gray` | Hints and placeholders |
| `accent` | `cyan` | Prompt prefix, metadata labels, search matches |
| `border` | `dark_gray` | Panel and page borders |
| `error` | `red` | Error messages in the search view |
| `bookmark_hover_foreground` | `white` | Text of the selected bookmark or search result |
| `bookmark_hover_background` | `blue` | Background of the selected bookmark or search result |
| `bookmark_page_color` | `dark_gray` | Page numbers in the bookmark tree and search results |
| `bookmark_hover_page_color` | `white` | Page number of the selected entry |
| `bookmark_expanded_color` | `white` | `[-]` marker of expanded bookmarks |
| `bookmark_collapsed_color` | `yellow` | `[+]` marker of collapsed bookmarks |
| `bookmark_leaf_color` | `dark_gray` | Bookmarks without children |
| `which_key_columns` | `3` | Columns of the which-key hint area |
| `which_key_foreground` | `white` | Which-key text and the completion list |
| `which_key_key` | `light_cyan` | Keys in which-key hints and the help popup |
| `which_key_description` | `light_magenta` | Descriptions in which-key hints and the help popup |
| `which_key_separator` | `" -> "` | Text between a key and its description |
| `which_key_separator_color` | `dark_gray` | Color of that separator |

These fields are accepted but currently unused: `focused_border`,
`selected_border`, `selected_foreground`, `selected_background`,
`hover_foreground`, `hover_background`, `hover_selected_foreground`,
`hover_selected_background`, `which_key_background`, and `which_key_rest`.
