#!/usr/bin/env bash
# Usage: reply.sh [--dry-run] <pr-number> <comment-id> <text>
# Does not resolve the thread: the thread id needs a separate GraphQL lookup.
set -euo pipefail

dry_run=0
if [ "${1:-}" = "--dry-run" ]; then
  dry_run=1
  shift
fi
if [ "$#" -ne 3 ]; then
  echo "usage: reply.sh [--dry-run] <pr-number> <comment-id> <text>" >&2
  exit 2
fi
pr=$1
comment_id=$2
text=$3

case "$pr" in
  '' | *[!0-9]*)
    echo "reply: PR number must be numeric" >&2
    exit 2
    ;;
esac
case "$comment_id" in
  '' | *[!0-9]*)
    echo "reply: comment id must be numeric" >&2
    exit 2
    ;;
esac
if ! printf '%s' "$text" | grep -q '[^[:space:]]'; then
  echo "reply: text is empty" >&2
  exit 1
fi
case "$text" in
  *$'\n'*)
    echo "reply: text must be one line" >&2
    exit 1
    ;;
esac
if printf '%s' "$text" | grep -qF "$(printf '\302\267')"; then
  echo "reply: text contains the middle dot character" >&2
  exit 1
fi

if [ "$dry_run" -eq 1 ]; then
  echo "gh api repos/{owner}/{repo}/pulls/$pr/comments/$comment_id/replies -f body=\"$text\""
  exit 0
fi

gh api "repos/{owner}/{repo}/pulls/$pr/comments/$comment_id/replies" -f body="$text"
