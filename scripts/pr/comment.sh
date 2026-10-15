#!/usr/bin/env bash
# Usage: comment.sh [--dry-run] <pr-number> <body-file>
set -euo pipefail

dry_run=0
if [ "${1:-}" = "--dry-run" ]; then
  dry_run=1
  shift
fi
if [ "$#" -ne 2 ]; then
  echo "usage: comment.sh [--dry-run] <pr-number> <body-file>" >&2
  exit 2
fi
pr=$1
body_file=$2

case "$pr" in
  '' | *[!0-9]*)
    echo "comment: PR number must be numeric" >&2
    exit 2
    ;;
esac
if [ ! -f "$body_file" ]; then
  echo "comment: file not found: $body_file" >&2
  exit 2
fi
if ! grep -q '[^[:space:]]' "$body_file"; then
  echo "comment: body is empty" >&2
  exit 1
fi
lines=$(awk 'END { print NR }' "$body_file")
if [ "$lines" -gt 10 ]; then
  echo "comment: body has $lines lines, the limit is 10" >&2
  exit 1
fi
if grep -qF "$(printf '\302\267')" "$body_file"; then
  echo "comment: body contains the middle dot character" >&2
  exit 1
fi

if [ "$dry_run" -eq 1 ]; then
  echo "gh pr comment $pr --body-file $body_file"
  exit 0
fi

gh pr comment "$pr" --body-file "$body_file"
