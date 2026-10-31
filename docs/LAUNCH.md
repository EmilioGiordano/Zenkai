# Launch plan

Goal: a first public release, `v0.1.0`, that a stranger from a forum can download, try on a
large file and trust with their data, and a repository a stranger can contribute to without
help.

Positioning: a native, free, cross-platform spreadsheet that opens, explores and cleans files
too large or too messy for the usual tools.

## Already in place

- Apache-2.0 license.
- CI on Windows, Linux and macOS: clippy and tests.
- Every crate at version `0.1.0`.

## Tasks

Each task is one branch. Order matters only where noted.

### L1. Refresh the compatibility report

- Regenerate `docs/COMPATIBILITY.md` on `main`.
- Done when: the report matches `main` and its date is recorded.

### L2. Decide preservation of unsupported content (after L1)

Today, saving drops charts, images and shapes, data validation, comments, hyperlinks, Excel
tables, AutoFilter and sheet protection, with a warning on open.

- Decide per item: preserve untouched on save before launch, or ship with it listed as a
  known limitation.
- Done when: the decision is recorded and every "preserve" item has its own task.

### L3. One workspace version

- Move the version to `[workspace.package]`; every crate inherits it.
- Show the version in the app (About) and in `--version` if the binary takes arguments.
- Done when: one edit changes the version everywhere.

### L4. Release workflow (after L3)

- GitHub Actions workflow triggered by a `v*` tag: release build on Windows, Linux and macOS,
  binaries attached to a GitHub release with the changelog section as notes.
- Windows binary unsigned; the README explains the SmartScreen prompt.
- Done when: a test tag on a fork produces a release with three downloadable binaries.

### L5. Changelog

- `CHANGELOG.md` in Keep a Changelog format, starting at `0.1.0`.
- Generated from commits (`git-cliff` or equivalent) and edited by hand before each release.
- Done when: the `0.1.0` section lists what a user gets, not internal work.

### L6. README for launch

- First screen: one-line positioning, a short GIF opening and scrolling a large file, a
  measurement table against Excel and LibreOffice on a file anyone can download.
- Download links, supported platforms (Linux and macOS marked experimental), known
  limitations from L2, a short note on how the project uses AI agents.
- Done when: a person who has never seen the project can install it and know what it does
  not do in under a minute. Builds on the pending README branch.

### L7. Split AGENTS.md

- Keep code rules public in `AGENTS.md`.
- Move the personal workflow (staging, local paths, disks, `scripts/pr`, board) to a separate
  maintainer file.
- Done when: a contributor reads `AGENTS.md` and finds only rules that apply to them.

### L8. CONTRIBUTING.md (after L7)

Sections:

1. Ways to contribute: bug reports with the failing file (anonymized), files that break the
   app for the corpus, code, docs.
2. Build and test on each platform; running the benchmark.
3. Scope: link to the roadmap and the non-goals; open an issue before a large PR.
4. Code rules: link to `AGENTS.md`.
5. AI-assisted contributions: welcome under the same rules and evidence; the author answers
   for the PR.
6. PR process: one branch per change, title format, CI green, who reviews.

- Done when: a PR from a fork can be validated by CI alone, without a local build by the
  maintainer.

### L9. Public roadmap and non-goals

- Short public `ROADMAP.md` (three or four milestones) and a non-goals list: macros and VBA,
  pivot tables, real-time collaboration, web version.
- Done when: both are linked from the README and CONTRIBUTING.

### L10. Issue templates and first issues

- Templates: bug report (version, platform, file), feature request (linked to the roadmap).
- Enable GitHub Discussions.
- 10 to 15 `good first issue` issues, scoped and described, reserved for people: agents do
  not take them.
- Done when: the issues exist and the templates appear on "New issue".

### L11. Launch content

- Technical post: how the grid renders millions of cells at 60 fps and how the engine was
  patched for scale.
- Done when: the post is ready and its numbers come from a release build.

## Launch order

1. r/rust and the GPUI and Zed community.
2. The technical post, linked from everywhere.
3. Show HN, once the first wave's bugs are fixed.
4. Communities with the problem: r/excel, r/dataengineering, r/datascience, r/linux.
5. Spanish-speaking developer communities and LinkedIn.

Read each community's self-promotion rules first. One community at a time, so a serious bug
does not burn all of them at once.

## After launch

- Reply to every issue within 48 hours.
- A release every two to four weeks, contributors named in the changelog.
- Bugs reported by users go first in the agents' queue.

## Not now

- Documentation website: `docs/` in markdown is enough until questions repeat or it grows
  past 10 to 15 pages; then mdBook from the same folder.
- Own Discord.
- Paid code signing.
