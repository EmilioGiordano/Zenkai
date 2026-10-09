# Vendored crates

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
- A formula that evaluates to an empty cell reads as 0 the first time too (`src/model.rs`,
  `evaluate_cell`). Tests: `src/test/test_empty_formula_result.rs`.
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

Zenkai-only, not for upstream:

- `build.rs` reports the crate version instead of asking `git describe`, which here
  would describe Zenkai's repository.
- The empty `[workspace]` table in `Cargo.toml`, so the crate builds on its own while
  living inside Zenkai's workspace directory.
- `LICENSE-MIT` and `LICENSE-Apache-2.0`, taken from the upstream repository root
  because the published crate does not include them.
