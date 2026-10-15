#!/usr/bin/env bash
# Usage: card.sh [--dry-run] <project-item-id> <tldr-file>
# Appends a TLDR paragraph (what the review found, how it was resolved) to a draft card.
set -eu

script_dir=$(cd "$(dirname "$0")" && pwd)
. "$script_dir/lib.sh"

dry_run=0
if [ "${1:-}" = "--dry-run" ]; then
  dry_run=1
  shift
fi
if [ "$#" -ne 2 ]; then
  echo "usage: card.sh [--dry-run] <project-item-id> <tldr-file>" >&2
  exit 2
fi
item=$1
tldr_file=$2
if [ ! -f "$tldr_file" ]; then
  echo "card: file not found: $tldr_file" >&2
  exit 2
fi

lines=$(awk 'END { print NR }' "$tldr_file")
if [ "$lines" -lt 1 ] || ! grep -q '[^[:space:]]' "$tldr_file"; then
  echo "card: TLDR is empty" >&2
  exit 1
fi
if [ "$lines" -gt 3 ]; then
  echo "card: TLDR has $lines lines, the limit is 3" >&2
  exit 1
fi
if grep -qF "$(printf '\302\267')" "$tldr_file"; then
  echo "card: TLDR contains the middle dot character" >&2
  exit 1
fi

append_to_draft_card "$dry_run" "$item" "TLDR: $(cat "$tldr_file")"
