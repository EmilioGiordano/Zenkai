# Maintainers

The maintainer's workflow for this repo: staging, local machine rules, PR mechanics, the
board, manual test, merge, and the agent roster. Contributor rules live in `AGENTS.md` and
apply here too; this file adds what only the maintainer and the maintainer's agents do.
Rules, not prose.

## Local files

- `DECISIONS.md`: every decision the spec does not cover, with date, options and reason. Local
  only: it is git-ignored, never commit it. Contributors record decisions in the PR's
  `## Decisions` section instead.

## Process

- At the end of each phase: stop, report what was done, what is pending and the risks, and
  wait for approval.
- Before closing a phase, in staging, run: `cargo fmt`, `cargo clippy` with no warnings,
  every test green, benchmark without regressions.

## Branches

- Only the lead pushes and opens PRs, through scripts/pr; merging needs the user's explicit approval.

## Pull requests

Agents write the content; `scripts/pr` (bash, `gh`, `jq`) does the GitHub mechanics. Use
`--dry-run` to preview. The title and body format are in `AGENTS.md`; these are the
maintainer additions.

- **Body:** the `## Validation` section also gives the staging result, then `Card: <url>` and,
  when stacked, `Stacked on #N`. `scripts/pr/lint.sh` enforces the whole format and
  `scripts/pr/open.sh` runs it before opening.
- **Review:** a summary of at most 6 lines and inline comments of at most 3 lines, each
  with an optional suggestion block. Low findings without action go only in the summary.
  The event is always COMMENT, never APPROVE or REQUEST_CHANGES: GitHub rejects
  REQUEST_CHANGES on the lead's own PR, so the verdict lives in the body, which starts
  with `[reviewer] VERDICT`. Findings on lines outside the PR diff go into the review body.
- **Thread replies:** one line: `Fixed in <sha>.`, `Won't fix: <reason>` or
  `False positive: <reason>`.
- **Staging comment, one per PR** (`scripts/pr/comment.sh`, at most 10 lines): gate result,
  measurements before and after, and what to test by hand.
- **Card:** every board card links its PR. `scripts/pr/open.sh --card <project-item-id>`
  appends `PR: <url>` to a draft card, or adds the PR to the project for an issue card.
  When the review closes, `scripts/pr/card.sh <project-item-id> <tldr-file>` appends a
  `TLDR:` of 2 or 3 lines to the card: what the review found and how it was resolved. On
  an issue card the TLDR goes as a comment on the issue.
- `scripts/pr/open.sh --card` and `card.sh` use project `ZENKAI_PROJECT_NUMBER` (default 2) owned
  by `ZENKAI_PROJECT_OWNER` (default EmilioGiordano).
- `scripts/pr/test.sh` checks the scripts in dry-run against local fixtures.

## Validation on the maintainer's machine

The development machine cannot build several branches at once, so work branches are never
compiled locally: no `cargo` (build, check, test, clippy, fmt), no benchmark. This replaces
the contributor commands in `AGENTS.md` for the maintainer's own branches.

- Branches are validated statically: self-review of every changed signature, call site,
  import and `match` against the code rules, tests written for the change, and the
  reviewers' static passes.
- Implementers may run `rustfmt --check` on the touched files only.
- The per-branch build is the PR's GitHub CI (`ci.yml`). The implementer's report lists
  what could not be verified without compiling and the commands staging must run.
- A red unrelated to the diff is noted on the PR in one line, naming the branch that fixes
  it, and is never fixed inside that PR.

Local builds happen only in staging, once per batch: one `cargo` process at a time, `-j 4`,
and no heavy measurement alongside a build.

A batch with a single branch runs the full gate directly in that branch's worktree and
copies the release exe from there; no staging branch is created.

A batch with several branches gets a fresh `staging/<date>` from `main` (adding `-n`
when the name is taken); the final tip of each branch is merged once with `--no-ff`. If
a branch changes after a failure, a new staging branch is created from `main` instead of
re-merging into the old one.

The full gate (commands in `AGENTS.md`, Validation) runs once per batch, in that order,
vendor suites included. The benchmark in `bench/` (release build) runs in the same batch when
it touches `crates/engine`, `crates/grid`, `crates/formats` or `bench/`. Nothing goes to the
user with any of these red.

## Workflow

Branches are developed in parallel, one agent and one worktree each, and validated together
in a staging branch.

1. **Design.** The rule is in `AGENTS.md` (Process); the maintainer approves the design artifact.
2. **Branch.** The implementer works per the code rules and validates statically per
   Validation above, without building. It commits in logical commits and never pushes.
   When the branch is ready for review, the lead pushes it and opens a draft PR with
   `scripts/pr/open.sh`.
3. **Static review, in parallel, no builds.** `quality-reviewer`, `security-reviewer` and
   `perf-auditor` read the diff against `AGENTS.md`, the security threat model and the
   `docs/SPEC.md` performance budgets. Fixes go on the branch as new commits.
   Each review and each fix round is posted on the PR (`scripts/pr/review.sh`,
   `scripts/pr/reply.sh`). The reviewers get the spec with decisions as plain statements,
   `AGENTS.md` and the diff: no reasons, no `DECISIONS.md`. The lead reconciles each finding against decisions.
4. **Staging.** `staging-integrator` (one agent, fresh context) validates the batch per
   Validation above: one branch is gated in its own worktree, several branches are merged
   one at a time with `--no-ff` into a fresh staging branch. After each merge it runs
   `cargo check --workspace --all-targets` to attribute breakage to a branch. Then, once,
   it runs the full gate, vendor suites included. It copies the release exe to
   `C:\Users\giord\Desktop\zenkai-test\zenkai.exe`, overwriting; if the file is locked it
   reports it and never kills the process. Staging uses the persistent target dir
   `D:/More-Code-Projects/Zenkai-compiled/staging`, which is never cleaned. When the batch
   enters staging the lead marks its PRs ready (`gh pr ready <n>`); the result is
   commented on each PR of the batch.
5. **Failures.** A fix for one branch goes back to the owning branch and is validated in
   a new staging round. A fix caused only by the combination goes in its own `fix/<topic>`
   branch, never on staging.
6. **Manual test.** The user tests the exe by hand and gives the OK in the session; every PR
   is the user's own, so GitHub cannot record an approval. Agents never drive the GUI or
   send input to the desktop.
7. **Merge.** On the user's approval the lead merges the PRs into `main` one at a time,
   in staging order, with merge commits (`--no-ff`); then `git diff staging/<date> main`
   must be empty. Staging is never merged into `main`. The lead pushes `main` and the merged
   work branches (`feat/*`, `fix/*`, `chore/*`, `perf/*`, `test/*`) with the user's approval,
   so every branch is kept on GitHub. `main` is pushed right after the merge: a PR opened
   against a stale `origin/main` lists commits that are not its own. Muse never pushes; never push `staging/*`, `night/*`
   or `worktree-agent-*`. Never merge to `main` before the manual test.

Long commands run in the background and the agent waits for them; it never hands back early.
After two failed review rounds on a branch, stop and ask the user instead of iterating.

## Agents and skills

- `.claude/agents/quality-reviewer.md`: blocking quality review, read-only.
- `.claude/agents/security-reviewer.md`: security review, threat model = a malicious file,
  read-only.
- `.claude/agents/perf-auditor.md`: static performance review on branches, benchmark in
  staging, read-only on code.
- `.claude/agents/test-engineer.md`: writes and runs tests.
- `.claude/agents/frontend-implementer.md`: implements UI in GPUI.
- `.claude/agents/engine-implementer.md`: implements engine work (Opus).
- `.claude/agents/implementer.md`: implements everything else: agent and MCP crates, CI.
- `.claude/agents/staging-integrator.md`: merges ready branches into `staging/<date>`, runs
  the full gate once and delivers the test exe.

Models: implementers and reviewers run on Sonnet, perf-auditor on Haiku, engine work on Opus.
Never delegate to the built-in general-purpose agent for implementation; it runs on the
session's model.
- `.claude/skills/review-changes/SKILL.md`: runs the reviewers in parallel and combines
  the verdict. It never fixes anything.
- `.agents/skills/zk-task/SKILL.md`: Muse workflow (`/zk-task`) turning a plain-words request into a TASK file on a fresh branch worktree, then stopping for approval.
- `.agents/skills/zk-branch/SKILL.md`: Muse workflow (`/zk-branch`) running a TASK file end to end: implement, static checks, parallel reviews, fixes, REPORT.
- `.agents/skills/zk-staging/SKILL.md`: Muse workflow (`/zk-staging`) merging ready branches into `staging/<date>`, running the full gate and benchmark once, delivering the exe and report.

Each reviewer has a "Known false positives" section. It starts empty and grows with every
review that was wrong; that is how the system learns. Record them there, not in this file.
