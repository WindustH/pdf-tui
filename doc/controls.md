# Controls

Key bindings are context aware. Browser actions are active while reading the
PDF. Input actions are active while the command prompt is open.

## Browser

- `q`, `ctrl-c`: quit
- `j`, `down`: move down
- `k`, `up`: move up
- `pgdn`: move down by a page-style step
- `pgup`: move up by a page-style step
- `h`, `left`: previous page
- `l`, `right`: next page
- `home`, `g g`: first page
- `end`, `G`: last page
- `L s`: switch to the default scroll layout
- `L g`: switch to the default grid layout
- `:`: open command prompt
- mouse wheel: move up or down

## Command Prompt

- `tab`: select the next completion candidate
- `shift-tab`: select the previous completion candidate
- `enter`: insert the selected completion when available, otherwise run the command
- `up`, `down`: browse command history for the current session
- `left`, `right`, `home`, `end`: move the cursor
- `ctrl-a`, `ctrl-e`: move to start or end
- `ctrl-u`, `ctrl-k`: delete before or after cursor
- `esc`: close the prompt

Prompt editing, completion selection, and command history are handled by
`framework-tui`, matching the interaction model used by `gallery-tui`.

## Which-Key

When a key sequence prefix is active, `pdf-tui` shows a which-key style hint
area above the status line.

Which-key layout and colors are configured in `theme.toml`.
