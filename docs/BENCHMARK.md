# Phase 0: engine benchmark

Date: 2026-10-08. Machine: the user's Windows 10 desktop (16 logical cores), release build,
Rust 1.99. Candidates: IronCalc 0.8.3 (`MIT OR Apache-2.0`) and logisheets-rs 1.16.1 (`MIT`).

## Recommendation

**IronCalc.** logisheets-rs could not open any of the five generated workbooks: a panic on
an empty `<dc:creator>` element (the default output of `rust_xlsxwriter`, one of the most
used xlsx writers), and after working around it, `ZipError(FileNotFound)` on every file.
Reliability is priority 1, so that alone decides it. IronCalc opened, recalculated, saved
and round-tripped every fixture with 100 % correct values and no loss of values,
formulas, bold flags or merged cells.

Risks that come with IronCalc, to manage rather than to block:

- **No incremental recalc.** `evaluate()` recomputes the whole workbook. An edit on the
  50k-formula file costs ~270 ms, over the 50 ms budget. Mitigation: recalc off the UI
  thread (done in the app), and later an upstream contribution or dependency-aware recalc.
- **Memory on open.** Peak heap of 1.3 GB for the 100k x 20 values file (13.7 MB on disk),
  336 MB after load. Above the "fraction of Excel" goal; needs work before 1.0.
- **Lookups and conditional aggregates do not scale.** Fixture 3 (20k rows of VLOOKUP,
  XLOOKUP, SUMIFS and COUNTIFS against 10k rows) takes 166 s to recalculate and 330 s
  after editing one input. Results are correct. Each criteria function scans its range
  per call and every edit recalculates everything, so this is the first thing to fix
  upstream or around the engine before such files are usable. This is the main risk of
  the choice.
- **`save_to_xlsx` panics** if the file cannot be created (`File::create(..).unwrap()`).
  Zenkai never calls it: it serializes with `save_xlsx_to_writer` and does its own atomic
  write.

## Method

`cargo run --release -p zenkai-bench -- run bench/fixtures [runs]` generates the fixtures
(seeded, reproducible) and measures each engine/fixture pair in its own process. Single
pieces: `generate <dir>`, `measure <engine> <file> <dir>`, `coverage`.

- Times: wall clock around the engine call. Open includes parsing; for IronCalc the full
  recalc is measured separately (`evaluate()`); logisheets recalculates on load.
- Memory: heap bytes through a counting global allocator (`peak_alloc`), not process RSS.
  Peak covers open plus first recalc; idle is the live heap right after.
- Correctness: the generator computes every formula result in Rust and the engine's value
  must match (relative tolerance 1e-9). These are generated files, so this stands in for
  "values cached by Excel" until real Excel files are added to `fixtures/real/`.
- Round trip: open, save to memory, reopen, compare every cell value and formula in the
  fixture extent. Formatting fixture also compares bold on every cell and merged ranges.

Deviations from the spec, to revisit:

- 1 run per pair instead of 5 (overnight time budget). Numbers below are single samples.
- The fixture 3 run overlapped with app compilation on the same machine.
- Column widths, borders and colors are not compared yet; only bold and merges.
- The Excel/LibreOffice memory reference is pending: it needs the user.

## Results

| Fixture | Engine | Open ms | Recalc ms | Edit ms | Save ms | Peak MB | Idle MB | Correct | Round trip | Bold | Merges |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 values 100k x 20 | IronCalc | 1929 | 371 | - | 1773 | 1314 | 336 | - | 2000000/2000000 | - | - |
| 2 simple formulas 50k | IronCalc | 1364 | 248 | 270 | 625 | 249 | 71 | 200000/200000 | 300000/300000 | - | - |
| 3 lookups 20k vs 10k | IronCalc | 994 | 165626 | 329901 | 394 | 114 | 36 | 80000/80000 | 150000/150000 | - | - |
| 4 chain 10k | IronCalc | 48 | 9 | 11 | 22 | 12 | 6 | 10000/10000 | 10000/10000 | - | - |
| 5 formatting 10k | IronCalc | 98 | 18 | - | 78 | 57 | 19 | - | 80000/80000 | 80000/80000 | 200/200 |
| all | logisheets | failed to open (see above) | | | | | | | | | |

Edit = change one input and get the dependents recalculated: A1 in the chain (A10000 must
become 10001), B1 in fixture 2, a category in fixture 3's data sheet.

## Function coverage

`zenkai-bench coverage` evaluates every function of the spec's minimum set plus the
operators in a fresh workbook and checks the result.

| | IronCalc | logisheets |
| --- | --- | --- |
| Passing | 70/70 | 69/70 |
| Failing | none | `XLOOKUP` returns `#NAME?` when typed (it may need the `_xlfn.` prefix) |
