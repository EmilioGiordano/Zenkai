---
name: review-changes
description: Review the current branch or uncommitted/untracked changes against Zenkai's quality, security and performance standards, using the reviewer subagents.
---

# Review Changes

Run Zenkai's reviewers against whatever is sitting in the repo: uncommitted and untracked
changes if there are any, otherwise the current branch against `main`. Report a combined
verdict.

This is the automated quality, security and performance pass before the user decides on
the merge. It is a proportionate check, not an exhaustive audit: one pass, no fan-out
beyond the agents listed here.

**This skill does not fix anything. It only reports.** Fixes are applied by
Zenkai-implementer after it receives the report.

## Arguments

An optional path, for work living in a git worktree other than the current directory,
for example `/review-changes ../zenkai-wt-grid`. Pass it through to every agent. With no
argument, they review the current directory.

## Steps

1. Resolve the target directory: the argument if given, otherwise the current directory.
   Do not compute the diff yourself; each agent determines its own target and reports
   which one it used. A branch that carries commits and also has uncommitted work is
   reviewed as the union of both.

2. Decide whether performance review applies: run `git diff main --name-only` in the
   target. If any changed path is under `crates/engine`, `crates/grid`, `crates/formats`
   or `bench/`, include `perf-auditor`.

3. Each agent gets only the target directory and what was asked (the task or the spec
   section, if the caller gave one). Never pass on what the implementer did, its reasons or
   its decisions: a reviewer that shares the implementer's model and reasoning makes the
   same mistakes and misses the same defects. Launch the agents in a single message so
   they run in parallel:
   - `Agent({ subagent_type: 'quality-reviewer', description: 'Quality review', prompt: 'Review <target directory> per your instructions and return the required report.' })`
   - `Agent({ subagent_type: 'security-reviewer', description: 'Security review', prompt: 'Review <target directory> per your instructions and return the required report.' })`
   - Only if step 2 applies: `Agent({ subagent_type: 'perf-auditor', description: 'Performance audit', prompt: 'Audit <target directory> per your instructions and return the required report.' })`
   - Only if the change touches `crates/engine`, `vendor/`, file reading or writing, or the
     agent security boundary (`crates/agent`, `crates/mcp-relay`, settings): a second
     opinion from a different model family, run read-only from the target directory, for
     example `gemini -p "<prompt>"` or `codex exec "<prompt>"`. The prompt asks for a
     review of `git diff main...HEAD` against `AGENTS.md` and the spec, reporting concrete
     defects with file and line, and contains nothing else. It sends the diff to that
     provider, so run it only where the user has agreed to that.

4. Wait for all of them. Do not summarize or act on one while another is still running,
   and never predict what a pending agent will say.

5. Present every report, then a combined verdict:
   - Overall **PASS** only if every agent comes back PASS (or PASS / ACCEPTED-RISK).
   - Otherwise **FAIL**, with every blocker, every Critical/High finding and every
     budget regression pulled into a single flat list ordered by severity.
   - Keep each finding's `CONFIRMED` / `PLAUSIBLE` mark visible. A `PLAUSIBLE` finding is
     a lead to check, not a defect to fix on sight.

6. Sanity-check the findings before relaying them. The agents run without the
   implementer's context on purpose, so they can flag a deliberate decision as a defect.
   If a finding contradicts a decision recorded in `DECISIONS.md`, say so next to it
   instead of dropping it silently; the user decides whether the decision still holds.

7. Do not commit, push, merge or open anything as part of this skill.
