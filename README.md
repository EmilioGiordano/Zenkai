# Zenkai

Native, lightweight, open-source desktop spreadsheet that opens and edits `.xlsx` files,
built in Rust with GPUI and designed to behave like Excel. The calculation engine is
[IronCalc](https://github.com/ironcalc/IronCalc) (chosen in Phase 0, see
`docs/BENCHMARK.md`).

- Product spec: `docs/SPEC.md`
- Rules for contributors and agents: `AGENTS.md`
- Decision log: `DECISIONS.md`
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
  Ctrl+= / Ctrl+- / Ctrl+0 zoom, Ctrl+wheel zoom, Ctrl+Shift+P command palette.
- Toolbar, formula bar with the active cell, sheet tabs, status bar with
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
- Columns resize by dragging header edges; double-click autofits. Insert/delete rows and
  columns, freeze panes, Go To (Ctrl+G / F5), fill down/right (Ctrl+D / Ctrl+R),
  AutoSum (Alt+=), today/now (Ctrl+; / Ctrl+Shift+;), find (Ctrl+F).
- Formula point mode: while typing a formula, arrows or clicks insert references, which
  are coloured in the formula and in the grid.
- Light, dark and high-contrast themes; follows the system light/dark setting until one is
  picked. Interface size (Ctrl+Alt+= / - / 0) is independent of the grid zoom.
- Screen readers: the sheet, the active cell (address, value, formula), the name box and
  the formula bar carry AccessKit labels.
- Diagnostics panel (Ctrl+Shift+D): memory, frame time, last recalculation.
- Autosave every minute to a recovery folder and a recovery offer after a crash.
- If saving fails (file locked by another program, read-only folder), Save As opens.
- Hostile files are rejected before the engine sees them (zip bombs, huge array areas,
  formulas deeper or longer than Excel allows).

## Build and run on Windows

Requirements: Windows 10/11, the MSVC build tools (Visual Studio Build Tools with the
"Desktop development with C++" workload) and rustup. The toolchain version is pinned in
`rust-toolchain.toml` and installs itself on the first build.

```powershell
cargo build --release -p zenkai -j 6
.\target\release\zenkai.exe                    # empty workbook
.\target\release\zenkai.exe path\to\book.xlsx  # open a file
```

`-j 6` keeps the first build (GPUI is large) from saturating the machine; drop it on a
dedicated build box. The release binary is `target\release\zenkai.exe`.

## Tests and checks

```powershell
cargo fmt --check
cargo clippy --all-targets -j 6 -- -D warnings
cargo test -j 6
cargo deny check            # cargo install cargo-deny --locked
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

License: Apache-2.0.
