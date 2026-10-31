# Roadmap

Where Zenkai is going. Dates are not promised; milestones are ordered by what builds trust first.

## Milestones

### 0.1 Trust (in progress)

Open, edit, recalculate and save `.xlsx`, `.csv` and `.tsv` without losing data.

- Shipped: atomic saves; unsupported content (charts, images, pivots, macros, comments,
  tables, validation) is kept by saves that do not rewrite its sheet, and a save that would
  drop it asks for a new name first; `.xls`, `.ods` and `.xlsb` open read-only with a notice.
- Remaining: downloadable builds. Windows has a portable zip and an installer built by
  script; automatic release builds for Windows, Linux and macOS are not in place yet, and
  Linux and macOS are only compiled and tested in CI.
- Known limit: content of a kind Zenkai does not detect may still be dropped when its sheet
  is rewritten.

### 0.2 Big files (in progress)

1M+ row sheets and very large CSV files open and scroll smoothly within a fraction of the
usual memory; switching sheets never freezes the UI.

- Shipped so far: streaming xlsx open, streaming CSV export, compact cell storage and
  hibernation of idle workbooks. Targets and measurements are in `docs/SPEC.md` (Phase 7).
- Remaining: confirm the targets on release builds with large real-world files, very large
  CSV import, and sheet switching on big workbooks.

### 0.3 Clean (proposed, needs approval)

Tools for messy data. Already available: sort A to Z / Z to A, find and replace, and the
`TRIM` formula function. Missing:

- Filter.
- Remove duplicates.
- Trim spaces on a selection, without a helper column.
- Split a column into several (text to columns).
- Fix column types (numbers and dates stored as text).

### Later

- Preserve, and then view, charts from existing files (Zenkai can already chart a selection).
- Edit conditional formatting.
- Write `.xls` and `.ods`.

## Non-goals

- Macros and VBA.
- Pivot tables.
- Real-time collaboration.
- A web or cloud version.
- Full Excel feature parity: Zenkai copies Excel's behavior where it has a feature, but does
  not aim to have every feature.
