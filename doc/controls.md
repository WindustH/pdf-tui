# Controls

These are the default bindings; all of them can be changed in
[`keymap.toml`](keymap.md). `F1` shows the bindings of the current view. `:`
opens the [command prompt](commands.md) in the viewer, metadata, and
bookmarks views.

## Viewer

| Keys | Action |
| --- | --- |
| `q`, `ctrl-c` | Quit |
| `j`, `down`, mouse wheel down | Scroll down one row (one slice in scroll layouts, one page row in grids) |
| `k`, `up`, mouse wheel up | Scroll up one row |
| `l`, `right`, `pgdn` | Move down one screen |
| `h`, `left`, `pgup` | Move up one screen |
| `g g`, `home` | First page |
| `G`, `end` | Last page |
| `r` | Reload the PDF from disk |
| `m` | Metadata view |
| `b` | Bookmarks view |
| `s` | Search view |
| `v` | Selection view |
| left click | Place a selection anchor (see [Selection](views.md#selection)) |
| `esc` | Cancel the selection anchors |
| `L s` | Switch to `scroll 1 3` and save it to `config.toml` |
| `L g` | Switch to `grid 2 2` and save it to `config.toml` |
| `F1` | Show key bindings |

The `next_page` and `previous_page` actions (move the focus by one PDF page)
have no default key.

## Metadata

| Keys | Action |
| --- | --- |
| `q`, `esc` | Back to the viewer |
| `ctrl-c` | Quit |
| `e` | Edit metadata in `$EDITOR` |
| `j`, `down`, mouse wheel down | Scroll down one line |
| `k`, `up`, mouse wheel up | Scroll up one line |
| `pgdn`, `pgup` | Scroll one screen |
| `F1` | Show key bindings |

## Bookmarks

| Keys | Action |
| --- | --- |
| `q`, `esc` | Back to the viewer |
| `ctrl-c` | Quit |
| `j`, `down`, mouse wheel down | Next visible bookmark |
| `k`, `up`, mouse wheel up | Previous visible bookmark |
| `pgdn`, `pgup` | Move one screen of bookmarks |
| `space` | Expand or collapse the selected bookmark |
| `z` | Expand all bookmarks, or collapse all when everything is expanded |
| `enter` | Jump to the selected bookmark |
| left click | Select a bookmark; clicking the selected one expands or collapses it |
| `h`, `left` / `l`, `right` | Narrow / widen the bookmark tree panel |
| `e` | Edit bookmarks in `$EDITOR` |
| `F1` | Show key bindings |

## Search

Typing edits the query; results update as you type.

| Keys | Action |
| --- | --- |
| `esc` | Back to the viewer |
| `ctrl-c` | Quit |
| `tab`, `down`, mouse wheel down | Next result |
| `shift-tab`, `up`, mouse wheel up | Previous result |
| `pgdn`, `pgup` | Move one screen of results |
| `enter` | Jump to the selected result |
| left click | Select a result; clicking the selected one jumps to it |
| `left`, `right`, `home`, `end`, `ctrl-a`, `ctrl-e`, `backspace`, `delete`, `ctrl-u`, `ctrl-k` | Edit the query |
| `F1` | Show key bindings |

## Selection

| Keys | Action |
| --- | --- |
| `q` | Back to the viewer |
| `esc` | Cancel the anchors of a new selection, otherwise back to the viewer |
| `ctrl-c` | Quit |
| `j`, `down`, `pgdn`, mouse wheel down | Next selection in the history |
| `k`, `up`, `pgup`, mouse wheel up | Previous selection |
| left click | Place an anchor of a new selection inside the shown one |
| `v` | Commit the new selection and show it |
| `y` | Copy the embedded text inside the selection |
| `Y` | Copy the selection as a PNG |
| `F1` | Show key bindings |

## Command Prompt

| Keys | Action |
| --- | --- |
| `enter` | Accept the highlighted completion, or run the command |
| `esc` | Close the prompt |
| `tab`, `shift-tab` | Next / previous completion |
| `up`, `down` | Previous / next command from this session's history |
| `left`, `right`, `home`, `end`, `ctrl-a`, `ctrl-e` | Move the cursor |
| `backspace`, `delete`, `ctrl-u`, `ctrl-k` | Delete before or at the cursor, before or after it |
| `F1` | Show prompt key bindings |

## Dialogs

- Confirmation of metadata or bookmark changes: `y` applies them; `enter`,
  `n`, `q`, or `esc` cancels.
- Key binding help: `F1`, `enter`, `esc`, or `q` closes it.

## Which-Key

After the first key of a multi-key binding (such as `L` or `g`), the footer
lists the keys that can follow. Its layout and colors are set in
[`theme.toml`](theme.md).
