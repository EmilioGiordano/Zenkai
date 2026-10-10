---
name: zk-staging
description: Merge ready branches into staging/<date> one at a time, run the full gate and benchmark once, deliver the release exe and a staging report.
---

# zk-staging

Turn a batch of ready branches into one tested build. Input: the list of ready
branches, each with a PASS REPORT (`/zk-staging <branch> [<branch> ...]`).

`.claude/agents/staging-integrator.md` is the source of truth for this skill:
read and follow it. The rules below restate its gates; on any conflict the
integrator brief wins.

Read `AGENTS.md` first; it is the rulebook.

Hard rules for the whole run:

- Never push, never merge into `main`. Merging staging into `main` happens only
  when the user explicitly says so after testing the exe, and the user pushes.
- Never open or drive the GUI, never send input to the desktop.
- Never kill a process, including a running `zenkai.exe`.
- One `cargo` process at a time, at most `-j 8` in total. If another `zk-branch`
  or `zk-staging` run is building, wait for it.
- Builds use the staging target dir below. Never clean it and never override
  `CARGO_TARGET_DIR`.

## Procedure

1. Create the worktree from the repo root: `git worktree add -b staging/<date>
   .claude/worktrees/staging main`. Add the untracked `.cargo/config.toml` with
   `target-dir = "D:/More-Code-Projects/Zenkai-compiled/staging"` (plus
   `rustc-wrapper = "sccache"`), and never change it afterwards.
2. Merge the branches one at a time, in the order given, with
   `git merge --no-ff <branch>`. After each merge run
   `cargo check --workspace --all-targets` to attribute breakage to the branch
   just merged.
3. Resolve a conflict only when the resolution is mechanical and keeps both
   intents. If it needs a design choice, abort with `git merge --abort`, skip
   the branch and report it.
4. Then once, in this order: `cargo fmt --check`,
   `cargo clippy --all-targets -- -D warnings`, `cargo test`,
   `cargo deny check`, `cargo build --release`. Plus the benchmark in `bench/`
   (release, as `README.md` documents it) when a merged branch touches
   `crates/engine`, `crates/grid`, `crates/formats` or `bench/`.
5. Copy the release exe to `C:\Users\giord\Desktop\zenkai-test\zenkai.exe`,
   overwriting. If the file is locked, report it and stop; never kill the
   process.

## Failures

A failure caused by one branch goes back to that branch: report the branch, the
command and the failing output, without fixing it on staging. A failure caused
only by the combination is fixed on the staging branch in its own commit.

## Report

Write `REPORT-staging-<date>.md` in the worktree root, at most 60 lines:
branches merged, gate results, exe path, and a manual test checklist for the
user.
