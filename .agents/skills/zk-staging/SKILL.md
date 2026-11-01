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

Read `AGENTS.md` and `MAINTAINERS.md` first; it is the rulebook.

Hard rules for the whole run:

- Never push, never merge into `main`. Merging into `main` happens only
  when the user explicitly says so after testing the exe; the lead pushes then.
- Never open or drive the GUI, never send input to the desktop.
- Never kill a process, including a running `zenkai.exe`.
- One `cargo` process at a time, `-j 4`, and no heavy measurement alongside a build.
  If another `zk-staging` run is building, wait for it.
- Builds use the staging target dir below. Never clean it and never override
  `CARGO_TARGET_DIR`.

## Procedure

1. One branch: run the full gate (step 4) directly in that branch's worktree
   and copy the exe (step 5). No staging branch, no staging worktree.
2. Several branches: create the worktree from the repo root:
   `git worktree add -b staging/<date> .claude/worktrees/staging main`
   (adding `-n` when the name is taken). Add the untracked `.cargo/config.toml`
   with `target-dir = "D:/More-Code-Projects/Zenkai-compiled/staging"` (plus
   `rustc-wrapper = "sccache"`), and never change it afterwards. Merge the
   final tip of each branch once, in the order given, with
   `git merge --no-ff <branch>`. After each merge run
   `cargo check --workspace --all-targets` to attribute breakage to the branch
   just merged. If a branch changes after a failure, start over with a new
   staging branch from `main`; never re-merge into the old one.
3. Resolve a conflict only when the resolution is mechanical and keeps both
   intents. If it needs a design choice, abort with `git merge --abort`, skip
   the branch and report it.
4. Then once, in this order: `cargo fmt --check`,
   `cargo clippy --all-targets -- -D warnings`, `cargo test`,
   `cargo deny check`, `cargo build --release`, then the vendor suites the workspace
   excludes, for each crate present under `vendor/` (`ironcalc_base`, and `ironcalc` once
   it exists): `cargo test --manifest-path vendor/<crate>/Cargo.toml`. Plus the benchmark in
   `bench/` (release, as `README.md` documents it) when a merged branch touches
   `crates/engine`, `crates/grid`, `crates/formats` or `bench/`.
5. Copy the release exe to `C:\Users\giord\Desktop\zenkai-test\zenkai.exe`,
   overwriting. If the file is locked, report it and stop; never kill the
   process.
6. Per-PR comment: for each PR of the batch, write a staging comment of at most 10 lines
   (gate result, measurements before and after, what to test by hand) and post it with
   `scripts/pr/comment.sh <n> <file>`, or leave the text in the report for the lead.
   Never post to a PR that is not in the batch.
7. After the manual test, on the user's approval: merge the branches (never
   staging) into `main` one at a time, in the same order, with `--no-ff`;
   then `git diff staging/<date> main` must be empty. Done by the user or by
   the lead with the user's explicit approval, as merge commits of the PRs in staging order;
   the lead then pushes `main` and the merged work branches (`feat/*`, `fix/*`, `chore/*`,
   `perf/*`, `test/*`) with the user's approval, so every branch is kept on GitHub.
   `main` is pushed right after the merge, before any new PR is opened.
   Muse never pushes; never push `staging/*`, `night/*` or `worktree-agent-*`.

## Failures

A failure caused by one branch goes back to that branch: report the branch, the
command and the failing output, without fixing it on staging. A failure caused
only by the combination is fixed in its own `fix/<topic>` branch, never on
staging.

## Report

Write `REPORT-staging-<date>.md` in the worktree root, at most 60 lines:
branches merged, gate results, exe path, and a manual test checklist for the
user.
