# Commands

Press `:` in the viewer, metadata, or bookmarks view to open the command
prompt. `tab` completes command names, and layout preset names after `layout`
or `layout-use`; `enter` accepts the highlighted completion or runs the
command; `up`/`down` browse this session's history. See
[Controls](controls.md#command-prompt) for all prompt keys.

| Command | Effect |
| --- | --- |
| `layout <name> [args...]` | Switch layout and save it to `config.toml` |
| `layout-use <name> [args...]` | Switch layout for this session only |
| `refresh` | Reload the PDF from disk |
| `metadata` | Metadata view |
| `bookmarks` | Bookmarks view |
| `search` | Search view |
| `selection` | Selection view |
| `help` | Show the key bindings of the current view |
| `write-config` | Save the current settings to `config.toml` |
| `clear-cache` | Delete cached pages, renders, and text indexes |
| `quit`, `q` | Quit |

`layout_use`, `write_config`, and `clear_cache` are accepted as aliases.

## `:layout <name> [args...]`

Switches to a layout preset and saves the choice as `layout.active` and
`layout.active_args` in `config.toml`, so it is used on the next start.

```text
:layout scroll 1 3
:layout scroll 2 3
:layout grid 2 3
:layout grid 2x3
```

The default presets are:

- `scroll <columns> <scroll_divisor>`: continuous scrolling; each page is cut
  into slices of at most 1/`scroll_divisor` of the screen height, and one
  scroll step moves one slice.
- `grid <rows> <columns>`: a fixed grid of whole pages. `2x3` is accepted in
  place of `2 3`.

Arguments are positional, in the order of the preset's `params` in
[`config.toml`](configuration.md#layout). Missing arguments keep the preset
defaults; extra arguments are an error. Rows, columns, and the divisor must be
between 1 and 64. The reading position is kept when the layout changes.

## `:layout-use <name> [args...]`

Same as `:layout`, without writing `config.toml`.

## `:refresh`

Reloads the PDF, its metadata, and its bookmarks, keeping the reading
position. Use it after another program rewrote the file, or enable
`behavior.auto_refresh` to do it automatically. The search index and the
selection history are dropped because they may no longer match.

## `:metadata`, `:bookmarks`, `:search`, `:selection`

Open the corresponding [view](views.md). The viewer keys are `m`, `b`, `s`, and
`v`.

## `:help`

Shows the key bindings of the current view, like `F1`.

## `:write-config`

Writes the current settings, including a layout chosen with `:layout-use` or
on the command line, to `config.toml`.

## `:clear-cache`

Deletes cached page and slice PNGs, terminal renders, search indexes, search
highlight and selection PNGs, and their LRU markers. Logs and remembered
reading positions are kept. The command is refused while another `pdf-tui`
instance uses the same cache.
