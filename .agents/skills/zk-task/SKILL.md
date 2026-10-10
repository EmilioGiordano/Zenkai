---
name: zk-task
description: Turn a plain-words request into a TASK file on a fresh branch worktree with its own target dir, then stop for user approval without implementing.
---

# zk-task

Turn the user's request, in plain words (`/zk-task <request>`), into a TASK file
ready for `/zk-branch`. Read `AGENTS.md` first; it is the rulebook.

## 1. Shape the task

Read the repo to find the code pointers the task needs. If the request is a UI
task, the user must point to an approved mockup or design artifact; if none
exists, stop and ask the user for one instead of guessing.

## 2. Create the worktree

From the repo root, on branch `main`:

```
git worktree add -b <type>/<topic> .claude/worktrees/<topic> main
```

`<type>` is one of `feat`, `fix`, `perf`, `test`, `chore`. Then write the
untracked file `.cargo/config.toml` in the new worktree:

```toml
[build]
rustc-wrapper = "sccache"
target-dir = "D:/More-Code-Projects/Zenkai-compiled/<topic>"
```

Never create or change that file afterwards, and never override
`CARGO_TARGET_DIR`. That target dir is persistent and is never cleaned.

## 3. Write the TASK file

Write `TASK-<topic>.md` in the worktree root with these sections:

- Goal: what the branch must achieve, in one paragraph.
- Approved design: the mockup or artifact (UI tasks), or "not a UI task".
- Code pointers: files found by reading the repo, with paths.
- Steps: the implementation order.
- Out of scope: what the branch must not touch.
- Validation: the branch checks (`cargo fmt`, `cargo check -p <crates>
  --all-targets`, `cargo test -p <crates>`, debug only) and what the user must
  check by hand in the staging exe.
- Git rules: one logical change per commit, English imperative subjects, no
  attribution, never push or merge, never commit the TASK or REPORT files.
- Final report: the branch ends with `REPORT-<topic>.md` (verdict, commits,
  checks, findings, risks, manual test needs).

## 4. Stop

Stop after writing the TASK file and ask the user to approve it. Never
implement in this skill; implementation is `/zk-branch`'s job.
