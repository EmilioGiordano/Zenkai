# Vendored crates

## ironcalc 0.8.3

The crates.io release of [IronCalc](https://github.com/ironcalc/IronCalc)'s import,
export and comparison shell (same 0.8.3 release as the engine below), with
Zenkai's changes on top. The first commit that adds it is the published crate
unchanged, so `git log -p -- vendor/ironcalc` shows only our changes. Zenkai
builds it in place of the crates.io release through `[patch.crates-io]` in the
root `Cargo.toml`. Run its tests with:

```powershell
cargo test --manifest-path vendor\ironcalc\Cargo.toml --target-dir target\vendor-ironcalc
```

Meant for upstream, each with tests. Unless noted, each stands on its own:

- The xlsx import refuses a worksheet or shared string part past the open
  budget before inflating it: a worksheet DOM holds about sixteen times its
  bytes (`src/import/mod.rs`, constants in `ironcalc_base::open_budget`).
- The xlsx export writes each worksheet row by row to the zip through
  `write_worksheet_xml` instead of building the whole part as one string, and
  exposes the other parts through `get_package_parts`, so a caller can write
  the package itself. An unevaluated formula now returns an error instead of
  panicking (`src/export/mod.rs`, `src/export/worksheets.rs`). Tests:
  `src/export/test/test_export.rs`.
- The xlsx export writes a run of the same formula down a column as an Excel shared
  formula (`<f t="shared" ref=".." si="..">` once, then `<f t="shared" si=".."/>`), so the
  formula is stringified once per run and the part is smaller (`src/export/worksheets.rs`).
  Tests: `src/export/test/test_export.rs`.
- `write_rows_xml` writes only the `<row>` elements a caller lists, with the shared formula
  encoding (`SharedFormula`) the caller chooses for each formula cell, so rows can be
  placed among rows copied from an existing part without clashing with its `si` numbers
  (`src/export/worksheets.rs`). Builds on the shared formula runs above, whose row writer
  it shares. The caller builds the row styles once with `row_styles` and passes them to every
  call, so a caller writing many spans does not rebuild the map per span. Tests:
  `src/export/test/test_export.rs`.
- The xlsx import skips comments and whitespace between the `<row>` elements of a sheet
  instead of failing on them as rows without an index (`src/import/worksheets.rs`). Test:
  `test_comment_and_whitespace_between_rows` in `tests/test.rs`.
- `get_shared_string_item_xml` writes one `<si>` element of the shared
  strings part, so a caller can append strings to an existing part
  (`src/export/shared_strings.rs`). Tests: `src/export/test/test_export.rs`.
- `get_style_items_xml` writes the font, fill, border, number format and cell format
  elements a styles part lacks past the first N of each list, so a caller can append
  styles to an existing part and keep every existing style index and byte
  (`src/export/styles.rs`; the whole-list writers now share the per-item writers). Tests:
  `src/export/test/test_export.rs`.

Zenkai-only, not for upstream:

- The empty `[workspace]` table in `Cargo.toml`, so the crate builds on its own
  while living inside Zenkai's workspace directory.
- The path-only `ironcalc` dev-dependency with the `mock_time` feature, which the published
  crate strips from `Cargo.toml`. Without it the generated tests compare templates holding
  `NOW()` and `TODAY()` against today's clock and fail as the date moves.

## ironcalc_base 0.8.3

The crates.io release of [IronCalc](https://github.com/ironcalc/IronCalc)'s engine
(release commit 9bed9ec), with Zenkai's changes on top. The first commit that adds it is
the published crate unchanged, so `git log -p -- vendor/ironcalc_base` shows only our
changes. Zenkai builds it in place of the crates.io release through `[patch.crates-io]` in
the root `Cargo.toml`. Run its tests with:

```powershell
cargo test --manifest-path vendor\ironcalc_base\Cargo.toml --target-dir target\vendor
```

Meant for upstream, each with tests. Unless noted, each stands on its own:

- Incremental recalculation: `src/dependency_index.rs`, `src/incremental.rs`,
  `src/user_model/recalculation.rs` and the hooks in `src/model.rs`, `src/new_empty.rs`,
  `src/lib.rs`, `src/user_model/{common,undo_redo,clipboard,mod}.rs`. Builds on
  ironcalc/IronCalc#1435. Tests: `src/test/test_incremental.rs`,
  `src/test/test_recalculation_fallbacks.rs`.
- `Model::full_evaluations` counts the evaluations of every formula, so a caller can tell
  whether an edit evaluated in full (`src/incremental.rs`, `src/model.rs`,
  `src/new_empty.rs`). Needs the incremental recalculation. Test:
  `full_evaluations_counts_only_full_evaluations` in `src/test/test_incremental.rs`.
- A formula that evaluates to an empty cell reads as 0 the first time too (`src/model.rs`,
  `evaluate_cell`). Tests: `src/test/test_empty_formula_result.rs`.
- A workbook with circular references keeps its dependency index, and an edit is
  recalculated incrementally unless a cell it changes is, or reads, a cell a cycle was
  found at, or the workbook has a formula that reads cells found at run time
  (INDIRECT, OFFSET, CELL, INFO, names) (`src/incremental.rs`,
  `src/dependency_index.rs`, `Model::circular_cells`). The cells a cycle taints are kept
  up to `MAX_TAINTED_CELLS`; past that, or past the walk limit, the overflow is remembered
  until the index is rebuilt and such edits are evaluated in full without walking again.
  Needs the incremental recalculation. Tests: `src/test/test_incremental_cycles.rs`,
  `dependents_of_stops_past_the_cells_allowed` in `src/dependency_index.rs`.
- Conditional formatting is re-evaluated after an incremental recalculation only for the
  rules, and for rules that look at one cell at a time only for the cells, that the
  changed cells can affect; each rule keeps what it gave to its cells and
  `get_extended_style_for_cell` merges them in priority order, so the results are those of
  a full evaluation (`src/cf_runs.rs`, `src/conditional_formatting.rs`,
  `src/dependency_index.rs`, `src/incremental.rs`). Needs the incremental recalculation.
  Tests: `src/cf_runs.rs`, `src/dependency_index.rs`,
  `src/test/conditional_formatting/incremental.rs`.
- Collecting the spill anchors reads each row's cells in column order without allocating
  a vector per row, and the walks that look for formula cells or dynamic anchors skip a
  sheet with no formulas (`src/model.rs`, `get_formula_cells`, `collect_spill_cells`,
  `src/incremental.rs`, `build_dependencies`). Stands on its own. Tests:
  `src/test/test_full_evaluation_cells.rs`.
- `Model::support` recorded only when the workbook has spills (`src/model.rs`,
  `record_support`).
- Hiding or showing rows recalculates what reads them, for SUBTOTAL
  (`src/user_model/common.rs`, `src/user_model/undo_redo.rs`). Needs the incremental
  recalculation: it records the row's cells as edited.
- Text criteria as in Excel: `<=` and `>=` compared the wrong way round, and `<>text`
  skipped non-text cells (`src/functions/util.rs`). Tests:
  `src/test/test_criteria_semantics.rs`.
- Text criteria matched without allocating per cell (`src/functions/util.rs`).
- SUMIF, COUNTIF and the rest of the family read each large range once per
  recalculation (`src/criteria_ranges.rs`, `src/functions/statistical/if_ifs.rs`). Tests:
  `src/test/test_criteria_ranges.rs`. Needs the incremental recalculation for
  `Model::circular_hits` and for turning the cache on around `evaluate_incremental`;
  without it, only the hooks in `Model::evaluate` remain.
- SUMIF and SUMIFS round the exact sum once instead of adding in order
  (`src/exact_sum.rs`), so the result does not depend on the order cells are added in;
  it can differ from upstream in the last bit. Tests: `src/exact_sum.rs`,
  `src/test/test_fn_sumifs.rs`.
- SUMIF, SUMIFS, COUNTIF and COUNTIFS keep their totals between recalculations and
  update them by what an edit changed instead of reading their ranges again
  (`src/criteria_totals.rs`, `src/criteria_ranges.rs`, `src/incremental.rs`,
  `Model::criteria_deltas`). Tests: `src/test/test_criteria_totals.rs`,
  `src/criteria_ranges.rs`. Needs the incremental recalculation, the exact sum and the
  per-evaluation criteria ranges above.
- COUNTIF and COUNTIFS walk the sheet for its used area only when a range is a whole
  row or column (`src/functions/statistical/if_ifs.rs`); the walk cost about 60 ms per
  call on a 3M-cell sheet. Covered by `src/test/test_criteria_semantics.rs`.
- COUNTIFS over a whole-sheet range like `A:XFD` counted its blank tail in i32, which
  overflowed (`src/functions/statistical/if_ifs.rs`). Tests:
  `src/test/test_criteria_semantics.rs`.
- `UserModel::range_clear_contents_of_areas` clears several areas of a sheet as one undo
  step, so a sparse selection clears only the cells it holds instead of every cell of its
  box; an array formula may span areas as long as it lies inside the given bounds
  (`src/user_model/common.rs`, `Model::arrays_inside` and `Model::clear_area_contents` in
  `src/model.rs`). Tests: `src/test/user_model/test_clear_areas.rs`.
- `Worksheet::sheet_data` is a `CellStore` (`src/cell_store.rs`) instead of nested
  `HashMap`s, so the cell layout can change without touching the callers; the importer
  and exporter of `ironcalc` use it too. No behaviour change, same bitcode encoding.
  Tests in the module.
- `CellStore` keeps each row as two sorted vectors (columns and cells) instead of a hash
  map per row, and `Cell` is 32 bytes instead of 80: the error detail of a formula value
  (`FormulaValue::error`) and the payloads of `Cell::ArrayFormula` and `Cell::SpillCell`
  (`Cell::new_array_formula`, `Cell::new_spill_cell`, `Cell::dynamic_range`,
  `Cell::spill_anchor`, `Cell::formula_value`) are boxed. A stored cell costs about 40 B
  instead of 147. The bitcode encoding of a `Cell` is unchanged (tests compare it with the
  inline fields); the encoding of a `CellStore` changed, so `.ic` files written by earlier
  builds do not load. Tests: `src/cell_store.rs` (random operations against the nested
  maps, size, encoding).
- `CellStore::append` moves the rows of another store into the first, so a caller that
  reads a sheet in pieces can build each piece in its own store and join them
  (`src/cell_store.rs`). Stands on its own. Test in the module.
- `Model::delete_columns` removes the `CellStore` rows it leaves without cells, so the used
  range matches the cells; rows that were already empty and row height, style and hidden
  state (kept in `Worksheet::rows`) stay. Needs the `CellStore` patch. IronCalc's own
  exporter writes only `sheet_data` rows, so an emptied row's height or style reaches the
  file only through Zenkai's save, which re-adds every `Worksheet::rows` entry; an upstream
  PR must export those rows too. Tests:
  `delete_columns_drops_rows_left_without_cells` in `src/test/test_actions.rs` and
  `delete_column_undo_redo_restores_cells_and_used_range` in
  `src/test/user_model/test_multi_row_column.rs`.
- `StaticResult` derives `Clone`, `Debug` and `PartialEq`, so a caller can copy a
  sheet's parsed formulas, for instance to save them away from the model
  (`src/expressions/parser/static_analysis.rs`).
- `UserModel::fill_formula` writes the formula of one cell over an area by storing the
  same formula index in each cell, instead of printing, parsing and storing a text per
  cell. It leaves the cells alone and returns false when a reference would leave the sheet
  or invert a range, or the area holds an array or a spill, so the caller fills as text.
  Undo restores the old cells; redo reads the source cell again (new `Diff::FillFormula`).
  `UserModel::begin_edit_group` and `end_edit_group` join the edits between them into one
  undo step, evaluating once (`src/user_model/fill_formula.rs`, `common.rs`). Tests:
  `src/test/user_model/test_fill_formula.rs`.
- The full evaluation lists the cells holding a formula instead of every cell, so a
  workbook of millions of values no longer builds a 12-byte index of each one first
  (`src/model.rs`, `get_formula_cells`). Stands on its own. Tests:
  `src/test/test_full_evaluation_cells.rs`.
- `UserModel::sheets_moved_by_undo` and `sheets_moved_by_redo` say which sheets the next
  undo or redo inserts or deletes rows or columns on, or `None` when the step also adds,
  removes or moves sheets, so a caller tracking row moves need not assume the worst on
  every replay (`src/user_model/history.rs`, `common.rs`). Tests:
  `src/test/user_model/test_sheets_moved.rs`.
- `UserModel::undo_cells` and `UserModel::redo_cells` tell which cells the next undo or
  redo step can change (`StepCells`: cell areas and the sheets whose rows or columns move,
  or `None` for the workbook; size, visibility, style, pane and sheet-setting diffs add
  or remove no cell), so a caller that caches per-sheet facts keeps the ones the step cannot touch
  (`src/user_model/history.rs`, `src/user_model/common.rs`). Stands on its own. Tests:
  `src/test/user_model/test_step_cells.rs`.
- NOW and TODAY read the clock once per evaluation, as Excel does, so `=NOW()-NOW()` is
  always 0 (`src/model.rs` `evaluation_milliseconds`, `src/incremental.rs`, `src/tz/`).
  Tests: `src/test/test_now.rs`. Stands on its own.
- `UserModel::to_bytes_with_history` and `UserModel::from_bytes_with_history` write and read the 
  workbook together with its undo and redo history, which `to_bytes` leaves out. The dump 
  refuses while an edit group is open or evaluation is paused (`src/user_model/common.rs`).
  `UserModel::write_with_history` hands the same bytes to a sink one encoded section at a time
  (each part of the workbook and each worksheet on its own, then the two histories), and
  `UserModel::from_reader_with_history` reads them back one section at a time, so neither side
  holds the whole dump.
  Stands on its own. Tests: `src/test/user_model/test_bytes_with_history.rs`.

Zenkai-only, not for upstream:

- `src/open_budget.rs` estimates the peak memory of opening a workbook (per cell,
  formula and row, measured with the `measure_open` instrument; one set for the
  streaming reader and a higher one for IronCalc's importer) and
  `Model::from_workbook` refuses with a needs-about message that also names the
  allowance, past sixty percent of RAM, instead of hanging the machine. Tests in
  the module.
- The `sysinfo` dependency in `Cargo.toml`, used by `open_budget` to read physical RAM.
- `build.rs` reports the crate version instead of asking `git describe`, which here
  would describe Zenkai's repository.
- The empty `[workspace]` table in `Cargo.toml`, so the crate builds on its own while
  living inside Zenkai's workspace directory.
- `LICENSE-MIT` and `LICENSE-Apache-2.0`, taken from the upstream repository root
  because the published crate does not include them.
