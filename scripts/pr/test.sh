#!/usr/bin/env bash
# Dry-run tests for the PR scripts. Never touches GitHub.
set -uo pipefail

dir=$(cd "$(dirname "$0")" && pwd)
fx=$dir/fixtures
failed=0

expect() {
  local want=$1 name=$2
  shift 2
  "$@" >/dev/null 2>&1
  local got=$?
  if [ "$got" -eq "$want" ]; then
    echo "ok   $name"
  else
    echo "FAIL $name (exit $got, wanted $want)"
    failed=$((failed + 1))
  fi
}

expect_output() {
  local pattern=$1 name=$2
  shift 2
  local output
  output=$("$@" 2>&1)
  if grep -q -- "$pattern" <<<"$output"; then
    echo "ok   $name"
  else
    echo "FAIL $name (missing: $pattern)"
    failed=$((failed + 1))
  fi
}

long_title="feat: $(printf 'a%.0s' $(seq 1 70))"
dot_title="feat: add $(printf '\302\267') thing"
two_lines=$(printf 'a\nb')

expect 0 "lint good" "$dir/lint.sh" "feat: add pr scripts" "$fx/good-body.md"
expect 0 "lint stacked title and decisions" "$dir/lint.sh" "feat: add pr scripts (2/3)" "$fx/good-body-decisions.md"
expect 1 "lint bad type" "$dir/lint.sh" "wip: add pr scripts" "$fx/good-body.md"
expect 1 "lint trailing period" "$dir/lint.sh" "feat: add pr scripts." "$fx/good-body.md"
expect 1 "lint uppercase" "$dir/lint.sh" "feat: Add pr scripts" "$fx/good-body.md"
expect 1 "lint long title" "$dir/lint.sh" "$long_title" "$fx/good-body.md"
expect 1 "lint dot title" "$dir/lint.sh" "$dot_title" "$fx/good-body.md"
expect 1 "lint bad section order" "$dir/lint.sh" "feat: add pr scripts" "$fx/bad-body.md"
expect 1 "lint dot body" "$dir/lint.sh" "feat: add pr scripts" "$fx/dot-body.md"
expect 1 "lint long body" "$dir/lint.sh" "feat: add pr scripts" "$fx/long-body.md"
expect_output "missing '## Why'" "lint reports missing section" "$dir/lint.sh" "feat: x" "$fx/long-body.md"
expect_output "Card" "lint reports missing card" "$dir/lint.sh" "feat: x" "$fx/long-body.md"

expect 0 "review good" "$dir/review.sh" --dry-run --files "$fx/pr-files.json" 5 "$fx/good-review.json"
expect_output '"event": "COMMENT"' "review is COMMENT" "$dir/review.sh" --dry-run --files "$fx/pr-files.json" 5 "$fx/good-review.json"
expect_output '"event": "COMMENT"' "review with High is still COMMENT" "$dir/review.sh" --dry-run --files "$fx/pr-files.json" 5 "$fx/blocking-review.json"
expect_output '.security-reviewer. FAIL' "review keeps the verdict in the body" "$dir/review.sh" --dry-run --files "$fx/pr-files.json" 5 "$fx/blocking-review.json"
expect_output '\[quality-reviewer\] PASS' "review body prefix" "$dir/review.sh" --dry-run --files "$fx/pr-files.json" 5 "$fx/good-review.json"
expect_output 'suggestion' "review suggestion block" "$dir/review.sh" --dry-run --files "$fx/pr-files.json" 5 "$fx/good-review.json"
expect 1 "review bad shape" "$dir/review.sh" --dry-run --files "$fx/pr-files.json" 5 "$fx/bad-review.json"
expect 1 "review middle dot" "$dir/review.sh" --dry-run --files "$fx/pr-files.json" 5 "$fx/dot-review.json"
expect_output "triple backtick" "review rejects a fence in a suggestion" "$dir/review.sh" --dry-run --files "$fx/pr-files.json" 5 "$fx/fence-review.json"
expect 2 "review non-numeric pr" "$dir/review.sh" --dry-run --files "$fx/pr-files.json" abc "$fx/good-review.json"

expect 0 "open good" "$dir/open.sh" --dry-run chore/x main "feat: add pr scripts" "$fx/good-body.md"
expect_output 'gh pr create --draft --base main --head chore/x' "open prints gh command" "$dir/open.sh" --dry-run chore/x main "feat: add pr scripts" "$fx/good-body.md"
expect 1 "open refuses main" "$dir/open.sh" --dry-run main main "feat: x" "$fx/good-body.md"
expect 1 "open refuses staging" "$dir/open.sh" --dry-run staging/2026-10-14 main "feat: x" "$fx/good-body.md"
expect 1 "open refuses night" "$dir/open.sh" --dry-run night/a main "feat: x" "$fx/good-body.md"
expect 1 "open refuses agent" "$dir/open.sh" --dry-run worktree-agent-1 main "feat: x" "$fx/good-body.md"
expect 1 "open runs lint" "$dir/open.sh" --dry-run chore/x main "bad title." "$fx/good-body.md"
for bad_branch in "HEAD:main" "+x:main" "refs/heads/main" "--receive-pack=x" "HEAD" "x:y" "@{-1}" "a..b"; do
  expect_output "refusing\|not a valid" "open refuses branch $bad_branch" "$dir/open.sh" --dry-run "$bad_branch" main "feat: x" "$fx/good-body.md"
  expect 1 "open exit status for branch $bad_branch" "$dir/open.sh" --dry-run "$bad_branch" main "feat: x" "$fx/good-body.md"
done
expect_output 'git push -u origin -- refs/heads/chore/x' "open pushes an explicit ref" "$dir/open.sh" --dry-run chore/x main "feat: add thing" "$fx/good-body.md"

expect 0 "reply good" "$dir/reply.sh" --dry-run 5 123 "Fixed in abc1234."
expect 1 "reply multi-line" "$dir/reply.sh" --dry-run 5 123 "$two_lines"
expect 2 "reply bad id" "$dir/reply.sh" --dry-run 5 x "Fixed in abc1234."
expect 2 "reply empty pr" "$dir/reply.sh" --dry-run "" 123 "Fixed in abc1234."
expect 2 "reply empty comment id" "$dir/reply.sh" --dry-run 5 "" "Fixed in abc1234."
expect 2 "reply non-numeric pr" "$dir/reply.sh" --dry-run x 123 "Fixed in abc1234."
expect 1 "reply empty text" "$dir/reply.sh" --dry-run 5 123 "  "

expect 0 "comment good" "$dir/comment.sh" --dry-run 5 "$fx/comment-good.md"
expect_output 'gh pr comment 5 --body-file' "comment prints the gh command" "$dir/comment.sh" --dry-run 5 "$fx/comment-good.md"
expect 1 "comment too long" "$dir/comment.sh" --dry-run 5 "$fx/comment-long.md"
expect 1 "comment middle dot" "$dir/comment.sh" --dry-run 5 "$fx/dot-body.md"
expect 2 "comment non-numeric pr" "$dir/comment.sh" --dry-run x "$fx/comment-good.md"

expect_output '"line": 42' "review keeps a commentable line inline" "$dir/review.sh" --dry-run --files "$fx/pr-files.json" 5 "$fx/good-review.json"
expect_output 'Not on the diff:.*crates/y.rs:7: Nit' "review moves a line outside the diff into the body" "$dir/review.sh" --dry-run --files "$fx/pr-files.json" 5 "$fx/good-review.json"
expect_output 'Not on the diff:.*crates/x.rs:1: Unchecked index' "review moves a line outside every hunk into the body" "$dir/review.sh" --dry-run --files "$fx/pr-files.json" 5 "$fx/blocking-review.json"
expect_output '"comments": ..' "review with no commentable line has no inline comments" "$dir/review.sh" --dry-run --files "$fx/pr-files.json" 5 "$fx/blocking-review.json"

expect 0 "card good" "$dir/card.sh" --dry-run PVTI_x "$fx/tldr-good.txt"
expect_output 'TLDR: Review found' "card prefixes TLDR" "$dir/card.sh" --dry-run PVTI_x "$fx/tldr-good.txt"
expect_output 'gh issue comment' "card comments on an issue card" "$dir/card.sh" --dry-run PVTI_x "$fx/tldr-good.txt"
expect 1 "card too long" "$dir/card.sh" --dry-run PVTI_x "$fx/tldr-long.txt"
expect 1 "card middle dot" "$dir/card.sh" --dry-run PVTI_x "$fx/tldr-dot.txt"

expect_output 'updateProjectV2DraftIssue' "open --card updates a draft card" "$dir/open.sh" --dry-run --card PVTI_x chore/x main "feat: add pr scripts" "$fx/good-body.md"
expect_output 'gh project item-add 2 --owner EmilioGiordano' "open --card adds an issue card to the project" "$dir/open.sh" --dry-run --card PVTI_x chore/x main "feat: add pr scripts" "$fx/good-body.md"
expect_output 'gh project item-add 7 --owner someone' "open --card honours the project env vars" env ZENKAI_PROJECT_NUMBER=7 ZENKAI_PROJECT_OWNER=someone "$dir/open.sh" --dry-run --card PVTI_x chore/x main "feat: add pr scripts" "$fx/good-body.md"
expect 2 "open --card without value" "$dir/open.sh" --dry-run --card

if [ "$failed" -eq 0 ]; then
  echo "all passed"
fi
[ "$failed" -eq 0 ]
