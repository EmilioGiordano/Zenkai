---
name: zk-branch
description: Run a Zenkai task branch end to end from a TASK file in a worktree: implement, branch checks, parallel reviews, fixes, and a REPORT file.
---

# zk-branch

Run one Zenkai task branch end to end. Invoke as `/zk-branch <TASK file path>`.
The TASK file lives in a git worktree, usually not the current directory.

Read `AGENTS.md` first; it is the rulebook. The `.claude/agents/` briefs below are
the source of truth for each role: reference them in child prompts, never paste
their content and never let a child inherit another agent's reasoning.

Hard rules for the whole run:

- Never push, merge, rebase, `reset --hard`, `git clean`, `branch -D`, or delete
  branches.
- Never open or drive the GUI, never send input to the desktop.
- Never commit `TASK-*.md` or `REPORT-*.md` files, nor anything outside the task.
- Builds use the worktree's `.cargo/config.toml` target dir. Never create or change
  that file and never override `CARGO_TARGET_DIR`.
- One `cargo` process at a time, at most `-j 8` in total.

## 1. Implement

Spawn one implementer subagent (fresh context) with the TASK path, the worktree
path, and `AGENTS.md`. It implements the TASK in that worktree, committing one
logical change per commit with English imperative subjects and no attribution.

Pick its brief by area and tell it to read and follow that file: UI work under
`crates/grid` or `crates/app` gets `.claude/agents/frontend-implementer.md`,
engine work under `crates/engine` or `vendor/` gets
`.claude/agents/engine-implementer.md`, everything else gets
`.claude/agents/implementer.md`.

## 2. Branch checks

In the worktree, on the touched crates only, debug builds only:

```
cargo fmt
cargo check -p <crates> --all-targets
cargo test -p <crates>
```

Never workspace-wide clippy or test, never `--release`, never the benchmark.

## 3. Review

Run `git diff main...HEAD` in the worktree. Spawn the reviewer subagents in
parallel, each with fresh context and only the TASK spec, `AGENTS.md`, the
worktree path, and that diff. Never pass the implementer's reasoning.

- Quality (`.claude/agents/quality-reviewer.md`): always.
- Security (`.claude/agents/security-reviewer.md`): when the diff touches
  downloads, archives, process launch, file parsing, or MCP.
- Performance (`.claude/agents/perf-auditor.md`): when the diff touches
  `crates/engine`, `crates/grid`, `crates/formats`, or `bench/`.

Each finding uses the format: `file:line`, severity, `CONFIRMED` or `PLAUSIBLE`,
defect, failure scenario.

## 4. Fix

Hand the findings to the implementer. It verifies each one, fixes the real ones,
rejects the wrong ones with a reason, and reruns the branch checks from step 2.

## 5. Rounds

At most 2 review rounds (steps 3-4). If round 2 still fails, stop and report.

## 6. Report

Write `REPORT-<topic>.md` in the worktree root, at most 60 lines: verdict,
commits, checks run with their results, each finding fixed or rejected with a
reason, open risks, and what needs a manual GUI test.
