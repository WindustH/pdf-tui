# Windows Setup

> **Experimental and untested:** the native Windows version has not been
> tested by the maintainer. GitHub Actions compiles the
> `x86_64-pc-windows-msvc` target, but successful compilation does not confirm
> that runtime dependencies, terminal graphics, editing, clipboard support, or
> every view work correctly on a real Windows installation. Please report
> Windows-specific problems in the project issue tracker.

This guide covers the native Windows executable. WSL users should run the Linux
build and follow the regular [Quick Start](quick-start.md) instead.

## Install The Executable

Download the `x86_64-pc-windows-msvc` ZIP from the
[pdf-tui releases](https://github.com/WindustH/pdf-tui/releases), extract it,
and add the directory containing `pdf-tui.exe` to `PATH`.

To build from source, install the Rust MSVC toolchain and the Microsoft C++
Build Tools, then run:

```powershell
git clone --recurse-submodules https://github.com/WindustH/pdf-tui.git
Set-Location pdf-tui
cargo build --release --locked
```

The resulting executable is `target\release\pdf-tui.exe`.

## Runtime Dependencies

`pdf-tui` uses several external programs. Only install the optional entries for
the features you need.

| Feature | Dependency | Requirement |
| --- | --- | --- |
| Open a document and read page geometry | `pdfinfo` from Poppler | Required for every raster backend |
| Recommended performance raster backend | `pdfium.dll` | Required when `pdf_raster_backend = "pdfium"` |
| Simplest fallback raster backend | `pdftoppm` from Poppler | Required when `pdf_raster_backend = "poppler"` |
| Alternative raster backend | `mutool` from MuPDF | Required when `pdf_raster_backend = "mutool"` |
| Embedded-text search | `pdftotext` from Poppler | Optional |
| Symbol/ASCII graphics fallback | `chafa` | Required when no native terminal image protocol works |
| Metadata view and editing | `exiftool` | Optional |
| Bookmark view and editing | `pdftk` | Optional |
| External metadata/bookmark editor | `sh` plus `$EDITOR` | Optional; see [Git, Shell, And Editor](#git-shell-and-editor) |

Pdfium is the default backend and was the fastest backend in the project's
existing Linux benchmark. It is therefore the recommended performance path.
This result does not establish its performance or reliability on Windows,
where none of the backends have been benchmarked or runtime-tested. A complete
high-performance Windows setup consists of Poppler for page geometry and
search, Pdfium for rasterization, and Chafa as a terminal graphics fallback.

The suggested installation order is:

1. Install Poppler. `pdfinfo` is required regardless of raster backend.
2. Install Pdfium and confirm that `pdfium.dll` can be loaded.
3. Install Chafa unless the chosen terminal's native image protocol is known to
   work.
4. Add ExifTool, PDFtk, MuPDF, Git/sh, and an editor only for the corresponding
   optional features.

Choose the dependency set that matches the features you need:

- Recommended: Poppler, Pdfium, and Chafa.
- Minimal diagnostic setup: Poppler and Chafa, using the Poppler backend.
- Full setup: add MuPDF for backend comparison, ExifTool for metadata, PDFtk
  for bookmarks, and Git/sh plus an editor for editing workflows.

### Install Poppler

Download a ZIP from the third-party
[Windows Poppler bundle](https://github.com/oschwartz10612/poppler-windows/releases),
extract it to a stable location such as `C:\Tools\poppler`, and add its
`Library\bin` directory to `PATH`. For the current PowerShell session:

```powershell
$env:Path = "C:\Tools\poppler\Library\bin;$env:Path"
```

Use the Windows environment-variable settings to add that directory to the
user `PATH` permanently. The bundle supplies `pdfinfo`, `pdftoppm`, and
`pdftotext`.

Check that the required commands are visible in a new PowerShell session:

```powershell
Get-Command pdf-tui, pdfinfo, pdftoppm, pdftotext
pdf-tui --version
pdfinfo -v
```

## Configuration And Cache Paths

The current Windows build reads `XDG_CONFIG_HOME` and `XDG_CACHE_HOME`. Set
them to the standard Windows roaming and local application-data roots before
the first run:

```powershell
$env:XDG_CONFIG_HOME = $env:APPDATA
$env:XDG_CACHE_HOME = $env:LOCALAPPDATA

[Environment]::SetEnvironmentVariable("XDG_CONFIG_HOME", $env:APPDATA, "User")
[Environment]::SetEnvironmentVariable("XDG_CACHE_HOME", $env:LOCALAPPDATA, "User")
```

Restart the terminal after setting persistent environment variables. The
resulting paths are:

- `%APPDATA%\pdf-tui\config.toml`
- `%APPDATA%\pdf-tui\keymap.toml`
- `%APPDATA%\pdf-tui\theme.toml`
- `%LOCALAPPDATA%\pdf-tui\` for page, render, search, selection, and log files

Without `XDG_CONFIG_HOME` or `HOME`, the application falls back to a relative
`.config\pdf-tui` directory. Explicitly setting the XDG variables avoids
configuration and cache locations changing with the working directory.

## Recommended Pdfium Backend

Pdfium avoids launching an external raster command for every page batch and is
the default backend. In the project's existing benchmark it was faster than
Poppler and MuPDF. This result is from Linux; native Windows performance remains
untested.

Download a compatible `pdfium-win-x64.tgz` from
[pdfium-binaries](https://github.com/bblanchon/pdfium-binaries/releases), then
extract it with 7-Zip or a recent Windows `tar`:

```powershell
New-Item -ItemType Directory -Force C:\Tools\pdfium | Out-Null
tar -xf .\pdfium-win-x64.tgz -C C:\Tools\pdfium
Get-ChildItem C:\Tools\pdfium -Recurse -Filter pdfium.dll
```

Use the path printed by the last command. Configure it directly:

```toml
[render]
pdf_raster_backend = "pdfium"
pdfium_library_path = "C:\\Tools\\pdfium\\lib\\pdfium.dll"
```

or set it as an environment variable:

```powershell
$env:PDF_TUI_PDFIUM_LIBRARY_PATH = "C:\Tools\pdfium\lib\pdfium.dll"
[Environment]::SetEnvironmentVariable(
  "PDF_TUI_PDFIUM_LIBRARY_PATH",
  "C:\Tools\pdfium\lib\pdfium.dll",
  "User"
)
```

The executable also checks `pdfium\lib\pdfium.dll` beside its own directory.
For a self-contained layout, place the files as follows:

```text
pdf-tui\
  pdf-tui.exe
  pdfium\
    lib\
      pdfium.dll
```

Poppler must remain installed because `pdfinfo` supplies page geometry and
`pdftotext` supplies embedded-text search.

## Poppler-Only Fallback

The Poppler-only setup is easier to diagnose because it does not load a dynamic
Pdfium library. Create a minimal configuration before the first run:

```powershell
$configDir = Join-Path $env:XDG_CONFIG_HOME "pdf-tui"
New-Item -ItemType Directory -Force $configDir | Out-Null

@'
[render]
pdf_raster_backend = "poppler"
'@ | Set-Content -Encoding ascii (Join-Path $configDir "config.toml")
```

`pdf-tui` fills in all missing default fields when it loads this file. Open a
document with a quoted path:

```powershell
pdf-tui "C:\Users\me\Documents\example.pdf"
```

## Optional MuPDF Backend

Download a Windows build from the
[MuPDF releases](https://mupdf.com/releases), add the directory containing
`mutool.exe` to `PATH`, and set:

```toml
[render]
pdf_raster_backend = "mutool"
mutool_bin = "mutool"
```

Verify it with `Get-Command mutool`. Poppler is still required for `pdfinfo`
and embedded-text search.

## Terminal Graphics

Use a modern terminal with mouse input and 24-bit color support. `img-tui`
probes the terminal and attempts Kitty, Sixel, iTerm2, then symbol/ASCII
rendering. Actual protocol support on native Windows has not been tested for
this project.

Install [Scoop](https://github.com/ScoopInstaller/Install) if the `scoop`
command is unavailable. Chafa upstream documents Windows package-manager
support, but package manifests can change names or repositories, so search the
current Scoop catalog first:

```powershell
scoop search chafa
```

Install the exact manifest returned by the search with `scoop install NAME`,
then verify it with `Get-Command chafa`. Alternatively, use a Windows package
or build described by the [Chafa project](https://github.com/hpjansson/chafa)
and put `chafa.exe` in `PATH`. Chafa is needed before forcing fallback modes:

```powershell
$env:GALLERY_TUI_RENDER_MODES = "symbols,ascii"
pdf-tui "C:\Users\me\Documents\example.pdf"
```

The environment variable name is shared with `gallery-tui` through `img-tui`.
If raw escape sequences appear, keep the fallback override and inspect the log
described below.

## Optional Tools And Editing

### ExifTool

Download the Windows executable package from the
[official ExifTool site](https://exiftool.org/). Keep its accompanying files
together, rename `exiftool(-k).exe` to `exiftool.exe`, move the package to a
stable directory, and add that directory to `PATH`:

```powershell
Get-Command exiftool
exiftool -ver
```

ExifTool enables metadata display and editing.

### PDFtk

Install a Windows `pdftk.exe` distribution, such as
[PDFtk Server](https://www.pdflabs.com/tools/pdftk-server/), and add its binary
directory to `PATH`:

```powershell
Get-Command pdftk
pdftk --version
```

The alternative [pdftk-java](https://gitlab.com/pdftk-java/pdftk) standalone
JAR requires Java and a `pdftk.cmd` wrapper because `pdf-tui` invokes a command
named `pdftk` directly. Without PDFtk, the viewer still opens but reports that
bookmarks are unavailable.

### Git, Shell, And Editor

The current external-editor integration launches `sh -c`. Native Windows
editing therefore requires a POSIX-compatible `sh.exe`. Git for Windows is one
way to provide it:

```powershell
winget install --id Git.Git -e
Get-Command sh
```

If `sh` is not found, add the Git directory containing `sh.exe` to `PATH`. Set
`EDITOR` to a command available from that shell, for example `nvim`:

```powershell
$env:EDITOR = "nvim"
[Environment]::SetEnvironmentVariable("EDITOR", "nvim", "User")
```

Close the editor after saving so `pdf-tui` can resume the terminal interface.

Metadata and bookmark tools are optional. Their absence is reported inside the
corresponding view and does not prevent page rendering.

## Known Windows Limitations

- The Windows version is untested and should currently be treated as
  experimental.
- Selected text and PNG clipboard copy do not yet have a native Windows
  clipboard backend. The existing implementation supports macOS and Linux
  clipboard commands.
- Metadata and bookmark editing depend on `sh`, which is not included with a
  default Windows installation.
- Native image protocol behavior depends on the terminal. Chafa symbol mode is
  the intended fallback.
- The CI release archive contains `pdf-tui.exe` and documentation, but does not
  bundle Poppler, Pdfium, MuPDF, Chafa, ExifTool, or PDFtk.

## Logs And Troubleshooting

The startup message prints the active log path. With the paths recommended
above, logs are under:

```text
%LOCALAPPDATA%\pdf-tui\logs\
```

On Windows, `latest.log` is a small text file containing the path of the newest
run log. Check that referenced path for missing executables, Pdfium load
failures, and terminal protocol errors.

Useful checks:

```powershell
Get-Command pdfinfo, pdftoppm, pdftotext
Get-Command chafa, exiftool, pdftk, mutool -ErrorAction SilentlyContinue
$env:PDF_TUI_PDFIUM_LIBRARY_PATH
$env:GALLERY_TUI_RENDER_MODES
```

If rendering still fails, start with the Poppler backend and Chafa fallback.
That removes Pdfium dynamic-library loading and native image protocol detection
from the initial diagnosis.
