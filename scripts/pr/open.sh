#!/usr/bin/env bash
# Usage: open.sh [--dry-run] [--card <project-item-id>] <branch> <base> <title> <body-file>
set -eu

script_dir=$(cd "$(dirname "$0")" && pwd)
. "$script_dir/lib.sh"

dry_run=0
card=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --dry-run)
      dry_run=1
      shift
      ;;
    --card)
      if [ "$#" -lt 2 ]; then
        echo "open: --card needs a project item id" >&2
        exit 2
      fi
      card=$2
      shift 2
      ;;
    *) break ;;
  esac
done
if [ "$#" -ne 4 ]; then
  echo "usage: open.sh [--dry-run] [--card <project-item-id>] <branch> <base> <title> <body-file>" >&2
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
  if [ -n "$card" ]; then
    link_pr_to_card 1 "$card" "<pr-url>"
  fi
  exit 0
fi

git push -u origin "$branch"
url=$(gh pr create --draft --base "$base" --head "$branch" --title "$title" --body-file "$body_file" | tail -n 1)
echo "$url"
if [ -n "$card" ]; then
  link_pr_to_card 0 "$card" "$url"
fi
