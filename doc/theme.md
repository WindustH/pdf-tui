# Theme

Theme settings are stored in:

- `~/.config/pdf-tui/theme.toml`

Colors accept named Ratatui colors, `reset`, `ansi:<index>`, or `#rrggbb`.

Base colors:

- `foreground`
- `background`
- `muted`
- `accent`
- `border`
- `focused_border`
- `selected_border`
- `selected_foreground`
- `selected_background`
- `hover_foreground`
- `hover_background`
- `hover_selected_foreground`
- `hover_selected_background`
- `error`

Which-key and completion colors:

- `which_key_columns`
- `which_key_background`
- `which_key_foreground`
- `which_key_key`
- `which_key_rest`
- `which_key_description`
- `which_key_separator`
- `which_key_separator_color`

The footer uses these fields for the status line, command prompt, completion
list, and which-key hints.
