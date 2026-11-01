# Development

Building, packaging, benchmarking and CI details. For setup on each platform and the
contribution rules, start with [CONTRIBUTING.md](../CONTRIBUTING.md).

## Build and run on Windows

Requirements: Windows 10/11, the MSVC build tools (Visual Studio Build Tools with the
"Desktop development with C++" workload) and rustup. The toolchain version is pinned in
`rust-toolchain.toml` and installs itself on the first build.

```powershell
cargo build --release -p zenkai -p zenkai-mcp -p zenkai-pdf-host -j 4
.\target\release\zenkai.exe                    # empty workbook
.\target\release\zenkai.exe path\to\book.xlsx  # open a file
```

`-j 4` keeps the first build (GPUI is large) from saturating the machine; drop it on a
dedicated build box. The release binary is `target\release\zenkai.exe`.

PDFs are read by `zenkai-pdf-host.exe`, a separate process, so a crash inside pdfium cannot take Zenkai down. It and `pdfium.dll` are loaded from the folder of `zenkai.exe` only.
Without either, everything else works and the PDF tab says the viewer is unavailable. To read
PDFs from a development build, build the host and put the pinned DLL next to the exe:

```powershell
powershell -File packaging\windows\fetch-pdfium.ps1 -Destination target\release
```

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

- `zenkai-<version>-windows-x64.zip`: portable, the exes plus `pdfium.dll`, `LICENSE` and `README.md`.
- `zenkai-<version>-windows-x64-setup.exe`: only when `iscc` (Inno Setup 6.3 or later)
  is on PATH. Installs per user without admin rights, adds a Start Menu entry, and
  optionally a desktop icon and an "Open with" entry for `.xlsx`, `.csv`, Markdown and PDF files (it never
  becomes the default program). The uninstaller keeps `%LOCALAPPDATA%\Zenkai` (recovery files).

The version comes from `crates/app/Cargo.toml`; the icon (`crates/app/zenkai.ico`) is a
placeholder. Release builds have no console window and log to
`%LOCALAPPDATA%\Zenkai\zenkai.log` (`RUST_LOG` sets the level).

The Windows build is unsigned. SmartScreen shows "Windows protected your PC": click "More info", then "Run anyway".

Release builds cover Windows x64, Linux x64 and macOS on Apple silicon (arm64) only. Viewing PDF files works only on Windows for now, because the PDF library is bundled only there.

## Tests and checks

Setup, `cargo fmt`, `clippy` and `test` are in [CONTRIBUTING.md](../CONTRIBUTING.md). Also run
`cargo deny check` (`cargo install cargo-deny --locked`). These run in staging and in CI, not
on work branches (see `MAINTAINERS.md`, Validation).

The calculation engine is IronCalc's `ironcalc_base` 0.8.3, vendored in
`vendor/ironcalc_base` with Zenkai's changes (incremental recalculation) and built in
place of the crates.io release through `[patch.crates-io]`. It sits outside the
workspace, so the commands above do not cover it; after changing it, also run its own
test suite, together with `vendor\ironcalc` once it exists, which is also outside the
workspace; staging runs every vendor crate present:

```powershell
cargo test -j 4 --manifest-path vendor\ironcalc_base\Cargo.toml --target-dir target\vendor
```

## Benchmark

```powershell
cargo build --release -p zenkai-bench -j 4
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

xlsx readers (IronCalc's and the fast one, which the Settings switch "Fast xlsx reader" or
`ZENKAI_XLSX_READER=fast` turns on):

```powershell
.\target\release\zenkai-bench.exe reader-bench big.xlsx 3        # median read/open time and peak memory of each
.\target\release\zenkai-bench.exe reader-compare fixtures\compat # field-by-field comparison of both
$env:ZENKAI_READER_CORPUS = "C:\files;D:\big.xlsx"; cargo test -p zenkai-engine --test reader_differential
```

## CI

GitHub Actions, in `.github/workflows/`:

- `ci.yml` (pull requests and pushes to `main`, skipped when only docs, scripts, Markdown,
  `.claude` or `.agents` change): draft PRs run `cargo fmt --check` and `cargo clippy
  --all-targets -- -D warnings` on Linux; ready PRs add `cargo test` and also run on Windows,
  plus the `vendor/` crates' own tests when the PR touches `vendor/`. A push to
  `main` only warms the cache on Linux and Windows: it runs clippy and builds the test artifacts
  (`cargo test --workspace --no-run`) for ready PRs to reuse; runs on `main` are never
  cancelled. Jobs time out after 30 minutes. macOS runs only from a manual dispatch.
- `deny.yml` (PRs that change `Cargo.toml`, `Cargo.lock` or `deny.toml`, weekly, and manual
  runs): `cargo deny check` on Linux.
- `bench.yml` (manual runs only): builds the benchmark in release on Windows and runs the
  baseline check above. Each run uploads its numbers as the `bench-current` artifact.

The baseline in `bench\baseline.csv` comes from the Phase 0 run on a desktop machine, not
from a CI runner, so the timing gate fails until it is refreshed: copy the `bench-current`
artifact of any `bench.yml` run (the artifact is uploaded even when the check fails) over
`bench\baseline.csv` and commit it.

## Pull requests

The scripts in `scripts/pr` (bash, `gh`, `jq`) open draft PRs, post reviews and reply in
threads; the standards are in `AGENTS.md`. `scripts/pr/test.sh` checks them in dry-run.
