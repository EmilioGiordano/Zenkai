#!/usr/bin/env bash
# Usage: review.sh [--dry-run] [--files <pr-files.json>] <pr-number> <review.json>
set -eu

# --files uses a saved copy of the PR files API response instead of fetching it.
dry_run=0
files_json=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --dry-run)
      dry_run=1
      shift
      ;;
    --files)
      if [ "$#" -lt 2 ]; then
        echo "review: --files needs a file" >&2
        exit 2
      fi
      files_json=$2
      shift 2
      ;;
    *) break ;;
  esac
done
if [ "$#" -ne 2 ]; then
  echo "usage: review.sh [--dry-run] [--files <pr-files.json>] <pr-number> <review.json>" >&2
  exit 2
fi
pr=$1
review_file=$2

case "$pr" in
  '' | *[!0-9]*)
    echo "review: PR number must be numeric" >&2
    exit 2
    ;;
esac
if [ ! -f "$review_file" ]; then
  echo "review: file not found: $review_file" >&2
  exit 2
fi

middle_dot=$(printf '\302\267')
if grep -qF "$middle_dot" "$review_file"; then
  echo "review: contains the middle dot character" >&2
  exit 1
fi

shape_errors=$(jq -r '
  def nonempty: type == "string" and length > 0;
  def line_count: split("\n") | length;
  if type != "object" then "review must be a JSON object" else
  [
    (if (.reviewer | nonempty) then empty else "reviewer must be a non-empty string" end),
    (if (.verdict | nonempty) then empty else "verdict must be a non-empty string" end),
    (if (.summary | nonempty) then empty else "summary must be a non-empty string" end),
    (if (.summary | type) == "string" and (.summary | line_count) > 6 then "summary has more than 6 lines" else empty end),
    (if (.findings | type) == "array" then empty else "findings must be an array" end),
    (if (.findings | type) == "array" then .findings[] | . as $f |
      (if ($f | type) != "object" then "finding must be an object" else
        (if ($f.path | nonempty) then empty else "finding path must be a non-empty string" end),
        (if ($f.line | type) == "number" and $f.line >= 1 and ($f.line | floor) == $f.line then empty else "finding line must be a positive integer (\($f.path))" end),
        (if ["Critical", "High", "BLOCKER", "Medium", "Low"] | index($f.severity) then empty else "finding severity must be Critical, High, BLOCKER, Medium or Low (\($f.path))" end),
        (if ($f.text | nonempty) then empty else "finding text must be a non-empty string (\($f.path))" end),
        (if ($f.text | type) == "string" and ($f.text | line_count) > 3 then "finding text has more than 3 lines (\($f.path):\($f.line))" else empty end),
        (if ($f.suggestion == null) or ($f.suggestion | type) == "string" then empty else "finding suggestion must be a string (\($f.path))" end),
        (if ($f.suggestion | type) == "string" and ($f.suggestion | contains("```")) then "finding suggestion contains a triple backtick (\($f.path):\($f.line))" else empty end)
      end) else empty end)
  ] | .[] end
' "$review_file") || {
  echo "review: not valid JSON" >&2
  exit 1
}
if [ -n "$shape_errors" ]; then
  echo "$shape_errors" | sed 's/^/review: /' >&2
  exit 1
fi

# GitHub rejects the whole review when an inline comment targets a line outside the diff,
# so such findings go into the review body instead.
if [ -z "$files_json" ]; then
  files_json=$(mktemp)
  trap 'rm -f "$files_json"' EXIT
  gh api --paginate "repos/{owner}/{repo}/pulls/$pr/files" | jq -s 'add' >"$files_json"
fi

payload=$(jq --slurpfile files "$files_json" '
  def right_lines:
    reduce (. // "" | split("\n"))[] as $row ({next: 0, lines: []};
      if ($row | startswith("@@")) then .next = ($row | capture("[+](?<start>[0-9]+)").start | tonumber)
      elif ($row | startswith("+")) or ($row | startswith(" ")) then .lines += [.next] | .next += 1
      else . end) | .lines;
  def placed($c): . as $f | (($c[$f.path] // []) | index($f.line)) != null;
  ($files[0] | map({key: .filename, value: (.patch | right_lines)}) | from_entries) as $commentable
  | ([.findings[] | select(placed($commentable))]) as $placed
  | ([.findings[] | select(placed($commentable) | not)]) as $unplaced
  | {
    event: "COMMENT",
    body: ("[\(.reviewer)] \(.verdict)\n\n\(.summary)"
      + (if ($unplaced | length) > 0
         then "\n\nNot on the diff:\n" + ($unplaced | map("- **\(.severity)** \(.path):\(.line): \(.text)") | join("\n"))
         else "" end)),
    comments: [$placed[] | {
      path: .path,
      line: .line,
      side: "RIGHT",
      body: ("**\(.severity)**: \(.text)"
        + (if .suggestion then "\n\n```suggestion\n\(.suggestion)\n```" else "" end))
    }]
  }
' "$review_file")

if [ "$dry_run" -eq 1 ]; then
  echo "gh api repos/{owner}/{repo}/pulls/$pr/reviews --input -"
  echo "$payload"
  exit 0
fi

echo "$payload" | gh api "repos/{owner}/{repo}/pulls/$pr/reviews" --input - | jq -r .html_url
