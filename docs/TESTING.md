# Testing beyond unit tests

Objective checks that do not depend on anyone's judgment. All of them run with `cargo test`.

## Property tests

| Invariant | Where |
| --- | --- |
| Edits followed by the same number of undos restore the workbook | `crates/engine/tests/undo_property.rs` |
| Open, edit, save, reopen keeps every input, value, style, size, hidden line and frozen pane | `crates/engine/tests/save_reopen_property.rs` |
| Incremental recalculation equals full recalculation | `crates/engine/tests/incremental_recalc.rs` |
| CSV written then imported returns the same values, for each delimiter and encoding | `crates/formats/tests/csv_round_trip.rs` |
| Open, create, edit, save, close, unload, quit and restart never lose unsaved work | `crates/app/src/documents/session_property.rs` |
| A settings.json change that gives agents more power is held until the user confirms | `crates/agent/tests/settings_escalation.rs` |

## Structured fuzzing on stable Rust

A valid document is damaged at random places (`test-support/json_mutation.rs` for JSON, part
and byte damage for xlsx) and the parser must answer with a result or an error, never a panic
or a hang.

| Input | Where |
| --- | --- |
| xlsx open path, preflight included, and save after a damaged open | `crates/engine/tests/fuzz_xlsx_open.rs` |
| CSV | `crates/formats/tests/fuzz_csv.rs` |
| session.json | `crates/app/src/documents/session_fuzz.rs` |
| settings.json | `crates/agent/tests/fuzz_settings.rs` |
| MCP JSON-RPC messages and tool calls | `crates/agent/tests/fuzz_mcp_message.rs` |
| Data generation spec JSON | `crates/datagen/tests/fuzz_spec.rs` |

The number of cases is the `cases` value in each file. Raise it for a long local run.

### Coverage-guided fuzzing (cargo-fuzz)

Needs a nightly toolchain, which this repository does not install.

```
rustup toolchain install nightly
cargo install cargo-fuzz --locked
```

Create `fuzz/` as its own workspace (add it to `exclude` in the root `Cargo.toml`, so
`cargo-deny` and the default build do not see `libfuzzer-sys`), one target per row of the
table above that calls the same function the test calls, seeded with the files in
`fixtures/compat/`. Run a target with `cargo +nightly fuzz run <target> -- -max_total_time=600`.

## Mutation testing

Needs `cargo install cargo-mutants --locked`. Run it on the modules a change touches:

```
cargo mutants -j 1 -p zenkai-agent --file crates/agent/src/settings.rs
cargo mutants -j 1 -p zenkai-datagen --file crates/datagen/src/budget.rs --file crates/datagen/src/plan.rs
```

A surviving mutant means a test is missing: add one that fails on it.

## Values computed by Excel

`fixtures/compat/` holds no file saved by Excel. The cached values in its `.xlsx` files are
the ones the generator (`zenkai-bench compat-generate`) was told to write, so comparing them
with the engine checks the corpus against our own expectations, not against Excel.

To compare with Excel, save these files with Excel (Windows, calculation mode automatic, so
every formula has a cached value) in `fixtures/compat/excel/`:

1. `functions.xlsx`: one sheet per family (math, text, date and time, lookup, logical,
   statistics, financial), each formula next to its arguments, including every error value
   (`#DIV/0!`, `#N/A`, `#VALUE!`, `#REF!`, `#NAME?`, `#NUM!`).
2. `dates-1904.xlsx`: the same dates in the 1900 and the 1904 date system.
3. `number-formats.xlsx` and a CSV of it saved from Excel (UTF-8): the CSV holds the text
   Excel shows, the xlsx the values behind it.
4. The same `functions.xlsx` saved from Excel for Mac, and from Excel with an es-AR regional
   setting.

The test then recalculates each file and compares every formula cell with its cached value,
with a relative tolerance of 1e-9, listing every mismatch.
