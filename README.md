# Zenkai

Native, lightweight, open-source desktop spreadsheet that opens and edits `.xlsx` files,
built in Rust with GPUI and designed to behave like Excel. The calculation engine is
[IronCalc](https://github.com/ironcalc/IronCalc) (chosen in Phase 0, see
`docs/BENCHMARK.md`).

- Product spec: `docs/SPEC.md`
- Rules for contributors and agents: `AGENTS.md`
- Ideas backlog: `docs/IDEAS.md`

## What works

- Open `.xlsx` (and `.csv`/`.tsv`), edit, recalculate, save atomically, Save As.
- Excel grid: number formats, fonts, bold/italic/underline/strike, fills, borders,
  alignment, column widths and row heights, merged cells, frozen panes, text overflowing
  into empty neighbours, `####` for numbers that do not fit.
- Excel keyboard: arrows, Ctrl+arrows, Shift to extend, Home, Ctrl+Home/End, PageUp/Down,
  Ctrl+A, Ctrl+Space, Shift+Space, F2, Enter/Tab (Enter returns to where Tab started),
  Esc, Delete, Ctrl+C/X/V (formulas shift on paste), Ctrl+Z/Y, Ctrl+S/O/N, F12, Ctrl+F,
  Ctrl+B/I/U, Ctrl+Shift+~ ! $ % # number formats, Ctrl+PageUp/PageDown, Shift+F11,
  Ctrl+= / Ctrl+- and Ctrl+wheel zoom, Ctrl+9 / Ctrl+0 hide rows / columns, Ctrl+Shift+P command palette.
- Optional sidebar (Ctrl+Alt+B) with spaces: named groups of open workbooks you can rename, delete,
  moved between with drag and drop or Alt+Up/Down, plus recent files. F6 moves focus into it and back.
- Session restore: closing keeps your spaces, open files, selection and scroll, and unsaved work
  (restored from the autosaved copy); on start only the active workbook loads, the rest load
  when you pick them. Files that disappeared show as not found and are never removed for you.
- Memory: past 60% of the physical memory (set ZENKAI_MEMORY_BUDGET_MB to change it) the workbooks unused for longest are
  unloaded one by one, only if saved and off screen; they reload when you open them.
- Search (Ctrl+E, which replaces Excel's Flash Fill): workbooks in your spaces, recent files and
  the sheets of open workbooks.
- Several workbooks open at once: Ctrl+Tab / Ctrl+Shift+Tab switch, Ctrl+W closes (asking about
  unsaved changes), Ctrl+Shift+T reopens the last closed one. Closing the last workbook leaves
  Zenkai open with a start view (New, Open, recent files), as Excel does; Ctrl+W is then a no-op.
- Toolbar, editable formula bar, sheet tabs, status bar with
  Average/Sum/Count of the selection.
- Live chart of the selection (Alt+F1): column, line or pie, export as SVG or copy as
  Mermaid.
- Unsupported content (charts, images, pivots, macros, comments, Excel tables, hyperlinks,
  validation, external links) is detected on open; saving asks for a new name instead of
  silently dropping it. `.xlsm` originals are never overwritten.
- Files the engine cannot open (`.xls`, `.ods`, `.xlsb`, broken `.xlsx`) open read-only
  with their values through calamine, with a clear notice.
- CSV/TSV import with a preview: detected encoding and separator, decimal comma
  (`1.234,56`) and day-first dates (`25/10/2026`) with switches, and the table shows the
  values exactly as they will be imported. Save As `.csv` exports with a BOM.
- Columns and rows resize by dragging header edges; double-click a column edge to
  autofit. Insert/delete rows and columns, freeze panes (or just the top row / first
  column), Go To (Ctrl+G / F5), fill down/right (Ctrl+D / Ctrl+R), AutoSum (Alt+=),
  today/now (Ctrl+; / Ctrl+Shift+;), find (Ctrl+F).
- Fill handle: drag the square at the selection corner to repeat it or continue a series
  (1, 2 → 3, 4; a date → the next days; "Item 9" → "Item 10").
- Sort A to Z / Z to A by the active cell's column; with one cell selected it sorts the
  current region (Ctrl+Shift+* selects it).
- Right-click menus on cells and sheet tabs, as in Excel; duplicate a sheet from its tab.
- Formatting: font and fill colour pickers, borders (bottom, all, outside, none;
  Ctrl+Shift+& / Ctrl+Shift+_), strikethrough (Ctrl+5), font size steps
  (Ctrl+Shift+> / <), increase/decrease decimal, wrap text and vertical alignment, clear
  formats or clear all.
- Numbers behave like Excel when they do not fit: General numbers lose decimals or switch
  to scientific notation, formatted numbers and dates widen a default-width column.
- Formula point mode: while typing a formula, arrows or clicks insert references, which
  are coloured in the formula and in the grid.
- Formula AutoComplete (Tab inserts the function), F4 cycles $ references (and repeats the
  last formatting outside the editor), Alt+Enter line breaks, Ctrl+Enter fills the selection, show formulas (Ctrl+`), find and
  replace (Ctrl+H), paste values (Ctrl+Shift+V), hide/unhide rows and columns.
- Format Cells (Ctrl+1) for number formats with a live sample; recent files in the
  command palette; drop a file on the window to open it.
- Light, dark and high-contrast themes; follows the system light/dark setting until one is
  picked. Interface size (Ctrl+Alt+= / - / 0) is independent of the grid zoom.
- Screen readers: the sheet, the active cell (address, value, formula), the name box and
  the formula bar carry AccessKit labels.
- Diagnostics panel (Ctrl+Shift+D): memory, frame time, last recalculation.
- Agent settings (Ctrl+,): `%APPDATA%\Zenkai\settings.json`, with
  `settings.schema.json` written next to it so an agent can edit it correctly. Changes on
  disk apply at once; a broken edit keeps the last valid settings and says why. The page
  detects Node, npx, Claude Code and Gemini CLI, adds the Claude, Gemini and Codex ACP
  presets, sets the permission mode and stores API keys in Windows Credential Manager.
- Autosave every minute to a recovery folder and a recovery offer after a crash.
- If saving fails (file locked by another program, read-only folder), Save As opens.
- Hostile files are rejected before the engine sees them (zip bombs, huge array areas,
  formulas deeper or longer than Excel allows).

## Build and run on Windows

Requirements: Windows 10/11, the MSVC build tools (Visual Studio Build Tools with the
"Desktop development with C++" workload) and rustup. The toolchain version is pinned in
`rust-toolchain.toml` and installs itself on the first build.

```powershell
cargo build --release -p zenkai -p zenkai-mcp -j 6
.\target\release\zenkai.exe                    # empty workbook
.\target\release\zenkai.exe path\to\book.xlsx  # open a file
```

`-j 6` keeps the first build (GPUI is large) from saturating the machine; drop it on a
dedicated build box. The release binary is `target\release\zenkai.exe`.

## Agents over MCP

Zenkai can let an MCP client, such as Claude Code, read and edit the workbook that is open,
live: changes recalculate, show at once and undo with Ctrl+Z; nothing is saved by the
agent. Turn on Settings (Ctrl+,) > "Allow MCP clients outside Zenkai", then register the
relay that ships next to `zenkai.exe` (the Settings page shows the exact command):

```powershell
claude mcp add zenkai -- "C:\path\to\zenkai-mcp.exe"
```

`zenkai-mcp.exe` only relays stdio to a named pipe that is random per run, limited to the
current user and guarded by a token written to `%LOCALAPPDATA%\Zenkai\mcp-endpoint.txt`
while the bridge runs. Tools: `list_workbooks`, `list_sheets`, `get_selection`,
`read_range`, `find`, `write_cells`, `set_formula`, `format_range`. Writes ask for
approval by default (Alt+Y allows; Enter or Esc denies); files downloaded from the internet keep
agents read-only, as Excel's Protected View does (Ctrl+Shift+E lifts it for the open
file). Cell content reaches the agent marked as
untrusted data.

## Packaging

```powershell
powershell -ExecutionPolicy Bypass -File packaging\windows\build.ps1
```

Builds with the `dist` profile (fat LTO, one codegen unit, stripped; slower than
`--release`, which stays the profile for development and the benchmark) and writes to
`dist\`:

- `zenkai-<version>-windows-x64.zip`: portable, the exe plus `LICENSE` and `README.md`.
- `zenkai-<version>-windows-x64-setup.exe`: only when `iscc` (Inno Setup 6.3 or later)
  is on PATH. Installs per user without admin rights, adds a Start Menu entry, and
  optionally a desktop icon and an "Open with" entry for `.xlsx` and `.csv` (it never
  becomes the default program). The uninstaller keeps `%LOCALAPPDATA%\Zenkai` (recovery files).

The version comes from `crates/app/Cargo.toml`; the icon (`crates/app/zenkai.ico`) is a
placeholder. Release builds have no console window and log to
`%LOCALAPPDATA%\Zenkai\zenkai.log` (`RUST_LOG` sets the level).

## Tests and checks

```powershell
cargo fmt --check
cargo clippy --all-targets -j 6 -- -D warnings
cargo test -j 6
cargo deny check            # cargo install cargo-deny --locked
```

The calculation engine is IronCalc's `ironcalc_base` 0.8.3, vendored in
`vendor/ironcalc_base` with Zenkai's changes (incremental recalculation) and built in
place of the crates.io release through `[patch.crates-io]`. It sits outside the
workspace, so the commands above do not cover it; after changing it, also run its own
test suite:

```powershell
cargo test -j 6 --manifest-path vendor\ironcalc_base\Cargo.toml --target-dir target\vendor
```

## Benchmark

```powershell
cargo build --release -p zenkai-bench -j 6
.\target\release\zenkai-bench.exe run bench\fixtures 1   # generates fixtures on first run
.\target\release\zenkai-bench.exe coverage               # function coverage table
```

Compatibility corpus (`docs/COMPATIBILITY.md`):

```powershell
.\target\release\zenkai-bench.exe compat-generate fixtures\compat
.\target\release\zenkai-bench.exe compat fixtures\compat docs\COMPATIBILITY.md
```

Fixture 3 (20k lookups against 10k rows) takes several minutes per engine; see
`docs/BENCHMARK.md`.

Regression check against the saved baseline (IronCalc, median of 5 runs; exits non-zero
if a time or memory metric is more than 15% worse or missing, or if any run computes wrong
values or loses data on round trip):

```powershell
.\target\release\zenkai-bench.exe check bench\fixtures bench\baseline.csv bench-current.csv
```

`bench-current.csv` has the same format as `bench\baseline.csv`; copy it over the
baseline to accept new numbers. Fixture 3 is not in the baseline: one run takes minutes.

## CI

GitHub Actions, in `.github/workflows/`:

- `ci.yml` (pushes to `main` and every pull request): `cargo fmt --check`, `cargo clippy
  --all-targets -- -D warnings` and `cargo test` on Windows, Linux and macOS, plus
  `cargo deny check` on Linux.
- `bench.yml` (pull requests that touch the engine, formats, grid, types or bench, and
  manual runs): builds the benchmark in release on Windows and runs the baseline check
  above. Each run uploads its numbers as the `bench-current` artifact.

The baseline in `bench\baseline.csv` comes from the Phase 0 run on a desktop machine, not
from a CI runner, so the timing gate fails until it is refreshed: copy the `bench-current`
artifact of any `bench.yml` run (the artifact is uploaded even when the check fails) over
`bench\baseline.csv` and commit it.

License: Apache-2.0.
