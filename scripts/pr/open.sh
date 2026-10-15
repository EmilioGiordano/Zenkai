#!/usr/bin/env bash
# Usage: open.sh [--dry-run] <branch> <base> <title> <body-file>
set -eu

script_dir=$(cd "$(dirname "$0")" && pwd)
dry_run=0
if [ "${1:-}" = "--dry-run" ]; then
  dry_run=1
  shift
fi
if [ "$#" -ne 4 ]; then
  echo "usage: open.sh [--dry-run] <branch> <base> <title> <body-file>" >&2
  exit 2
fi
branch=$1
base=$2
title=$3
body_file=$4

case "$branch" in
  main | staging/* | night/* | worktree-agent-*)
    echo "open: refusing to open a PR from '$branch'" >&2
    exit 1
    ;;
esac

"$script_dir/lint.sh" "$title" "$body_file"

if [ "$dry_run" -eq 1 ]; then
  echo "git push -u origin $branch"
  echo "gh pr create --draft --base $base --head $branch --title \"$title\" --body-file $body_file"
  exit 0
fi

git push -u origin "$branch"
gh pr create --draft --base "$base" --head "$branch" --title "$title" --body-file "$body_file"
