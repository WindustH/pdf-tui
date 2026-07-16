# pdf-tui

`pdf-tui` is a terminal PDF reader built with `framework-tui` and `img-tui`.

Dependencies are managed as git submodules:

```sh
git submodule update --init --recursive
```

```sh
pdf-tui /path/to/file.pdf
pdf-tui --progress 0.0 /path/to/file.pdf
pdf-tui /path/to/file.pdf scroll 1 3
pdf-tui /path/to/file.pdf scroll 2 3
pdf-tui /path/to/file.pdf grid 2 3
```

Runtime commands are available with `:`:

```text
layout scroll <columns> <scroll_divisor>
layout grid <rows> <columns>
clear-cache
quit
```

Full documentation is in [doc/index.md](doc/index.md).
