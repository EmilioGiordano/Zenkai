# Contributing to Zenkai

Thanks for helping. Zenkai is a native, lightweight desktop spreadsheet in Rust and GPUI
that opens, edits and saves `.xlsx` files as close to Excel as possible.

## Ways to contribute

- **Bug reports.** Say the Zenkai version, your platform and the steps. If a file triggers
  the bug, attach it after anonymizing it: replace names, amounts and any private text, and
  check that the problem still happens.
- **Files that break the app.** A file Zenkai cannot open, shows wrongly or saves badly is
  valuable even without a fix. Anonymized files may be added to the compatibility corpus.
- **Code.** Fixes and features that fit the scope below.
- **Docs.** Corrections and clearer wording in `README.md` and `docs/`.

## Build and test

You need [rustup](https://rustup.rs). The toolchain version is pinned in
`rust-toolchain.toml` and installs itself on the first build.

- **Windows:** Visual Studio Build Tools with the "Desktop development with C++" workload.
- **Linux:** install the GPUI system packages:
  ```sh
  sudo apt-get install -y --no-install-recommends pkg-config libxkbcommon-dev \
    libxkbcommon-x11-dev libxcb1-dev libwayland-dev libfontconfig1-dev \
    libfreetype-dev libvulkan1
  ```
- **macOS:** Xcode with the Metal toolchain (`xcodebuild -downloadComponent MetalToolchain`
  if `xcrun -sdk macosx metal --version` fails).

Then, on any platform:

```sh
cargo run --release -p zenkai -- path/to/book.xlsx
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The first build is slow because GPUI is large; add `-j 4` to keep the machine usable. CI runs
the same checks on Linux, Windows and macOS, so a fork PR can be validated by CI alone.

The vendored engine in `vendor/ironcalc_base` is outside the workspace. If you change it, also
run `cargo test --manifest-path vendor/ironcalc_base/Cargo.toml`.

To run the benchmark (needed when a change touches the engine, grid or file formats):

```sh
cargo build --release -p zenkai-bench
./target/release/zenkai-bench run bench/fixtures 1
```

On Windows the binary is `target\release\zenkai-bench.exe`.

## Scope

Read the [roadmap](docs/ROADMAP.md) and its non-goals (macros and VBA, pivot tables,
real-time collaboration, a web version, full Excel parity). Features outside the roadmap
are unlikely to be merged. Open an issue before a large PR so we can agree on the approach
first; small fixes can go straight to a PR.

## Code rules

The rules for code, tests, comments and commits are in [AGENTS.md](AGENTS.md). Key points:
no `unwrap` or `panic!` on paths reachable by user data, no `unsafe`, no comments that
narrate the code, clippy with no warnings. Anything that could lose or corrupt user data
needs a test.

## AI-assisted contributions

Welcome, under the same rules and the same evidence as any other change: tests that pass,
clean clippy, a diff you have read. The author answers for the PR, including every line an
assistant wrote, and must be able to explain it in review.

## Pull requests

- One branch per change. Name it `feat/<topic>`, `fix/<topic>`, `perf/<topic>`,
  `test/<topic>` or `chore/<topic>`.
- Commits are small, in English and imperative (`Fix range selection past frozen panes`).
- The PR title is `<type>: <imperative summary>`, with type one of feat, fix, perf, chore,
  test or docs, at most 72 characters and no trailing period.
- The body has `## Why`, `## What`, an optional `## Decisions` and `## Validation`
  (what you ran and what you did not verify).
- CI must be green. The maintainer reviews every PR and merges it.
