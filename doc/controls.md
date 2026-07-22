# Controls

Key bindings are context aware. Viewer actions are active while reading the
PDF. Metadata actions are active in the metadata view. Bookmark actions are
active in the bookmarks view. Selection actions are active in the selection
view. Input actions are active while the command prompt is open.

## Viewer

- `q`, `ctrl-c`: quit
- `f1`: show viewer key bindings
- `j`, `down`: move down
- `k`, `up`: move up
- `pgdn`: move down by a page-style step
- `pgup`: move up by a page-style step
- `h`, `left`: move up by a page-style step
- `l`, `right`: move down by a page-style step
- `home`, `g g`: first page
- `end`, `G`: last page
- `r`: refresh the current PDF from disk
- `m`: open PDF metadata
- `b`: open PDF bookmarks
- `s`: search embedded PDF text
- `v`: open selection history
- mouse left press: mark the first PDF selection corner
- mouse left drag release: mark the opposite corner
- `esc`: cancel an active selection anchor
- `L s`: switch to the default scroll layout
- `L g`: switch to the default grid layout
- `:`: open command prompt
- mouse wheel: move up or down

## Metadata

- `q`, `esc`: return to the viewer
- `f1`: show metadata key bindings
- `e`: edit PDF metadata in `$EDITOR`
- `j`, `down`: scroll metadata down
- `k`, `up`: scroll metadata up
- `pgdn`: scroll metadata down by one viewport
- `pgup`: scroll metadata up by one viewport
- `ctrl-c`: quit
- `:`: open command prompt

## Bookmarks

- `q`, `esc`: return to the viewer
- `f1`: show bookmark key bindings
- `e`: edit PDF bookmarks in `$EDITOR`
- `j`, `down`: move to the next visible bookmark
- `k`, `up`: move to the previous visible bookmark
- `pgdn`: move down by one bookmark viewport
- `pgup`: move up by one bookmark viewport
- `space`: expand or collapse the hovered bookmark
- `z`: expand all bookmarks, then collapse all bookmarks on the next press
- `enter`: jump to the hovered bookmark
- `h`, `left`: narrow the bookmark tree panel
- `l`, `right`: widen the bookmark tree panel
- `ctrl-c`: quit
- `:`: open command prompt

## Search

- type in the top search box to search embedded PDF text
- `esc`: return to the viewer
- `f1`: show search key bindings
- `tab`, `down`: move to the next search result
- `shift-tab`, `up`: move to the previous search result
- `pgdn`: move down by one result viewport
- `pgup`: move up by one result viewport
- `enter`: jump to the selected result
- `ctrl-c`: quit

## Selection

- `q`, `esc`: return to the viewer
- `f1`: show selection key bindings
- `v`: commit a child selection draft, or prepare to create one
- mouse left press: mark the first child-selection corner
- mouse left drag release: mark the opposite child-selection corner
- `j`, `down`, `pgdn`: move to the next session selection
- `k`, `up`, `pgup`: move to the previous session selection
- `y`: copy embedded text inside the selection
- `Y`: copy a newly rendered PNG of the selection
- mouse wheel: browse selection history
- `ctrl-c`: quit

## Command Prompt

- `tab`: select the next completion candidate
- `shift-tab`: select the previous completion candidate
- `enter`: insert the selected completion when available, otherwise run the command
- `up`, `down`: browse command history for the current session
- `left`, `right`, `home`, `end`: move the cursor
- `ctrl-a`, `ctrl-e`: move to start or end
- `ctrl-u`, `ctrl-k`: delete before or after cursor
- `esc`: close the prompt
- `f1`: show input key bindings

Prompt editing, completion selection, and command history are handled by
`framework-tui`, matching the interaction model used by `gallery-tui`.

## Which-Key

When a key sequence prefix is active, `pdf-tui` shows a which-key style hint
area above the status line.

Which-key layout and colors are configured in `theme.toml`.
