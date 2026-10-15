#!/usr/bin/env bash
# Usage: lint.sh <title> <body-file>. Prints one message per violation.
set -u

if [ "$#" -ne 2 ]; then
  echo "usage: lint.sh <title> <body-file>" >&2
  exit 2
fi
title=$1
body_file=$2
if [ ! -f "$body_file" ]; then
  echo "body file not found: $body_file" >&2
  exit 2
fi

failures=0
fail() {
  echo "lint: $1"
  failures=$((failures + 1))
}

bare_title=$title
stack_suffix_re=' \([0-9]+/[0-9]+\)$'
if [[ $title =~ $stack_suffix_re ]]; then
  bare_title=${title% (*}
fi
title_re='^(feat|fix|perf|chore|test|docs): [a-z].*[^.]$'
if ! [[ $bare_title =~ $title_re ]]; then
  fail "title must match: <type>: <imperative summary> (type feat|fix|perf|chore|test|docs, lowercase start, no trailing period)"
fi
if [ "${#title}" -gt 72 ]; then
  fail "title is ${#title} characters, the limit is 72"
fi

middle_dot=$(printf '\302\267')
if printf '%s' "$title" | grep -qF "$middle_dot"; then
  fail "title contains the middle dot character"
fi
if grep -qF "$middle_dot" "$body_file"; then
  fail "body contains the middle dot character"
fi

line_of() {
  grep -n -m1 -x "$1" "$body_file" | cut -d: -f1
}
why=$(line_of '## Why')
what=$(line_of '## What')
decisions=$(line_of '## Decisions')
validation=$(line_of '## Validation')

[ -n "$why" ] || fail "body is missing '## Why'"
[ -n "$what" ] || fail "body is missing '## What'"
[ -n "$validation" ] || fail "body is missing '## Validation'"
if [ -n "$why" ] && [ -n "$what" ] && [ "$why" -gt "$what" ]; then
  fail "'## Why' must come before '## What'"
fi
if [ -n "$what" ] && [ -n "$validation" ] && [ "$what" -gt "$validation" ]; then
  fail "'## What' must come before '## Validation'"
fi
if [ -n "$decisions" ]; then
  if [ -n "$what" ] && [ "$decisions" -lt "$what" ]; then
    fail "'## Decisions' must come after '## What'"
  fi
  if [ -n "$validation" ] && [ "$decisions" -gt "$validation" ]; then
    fail "'## Decisions' must come before '## Validation'"
  fi
fi

if ! grep -qE '^Card: https?://[^ ]+$' "$body_file"; then
  fail "body needs a 'Card: <url>' line"
fi

lines=$(awk 'END { print NR }' "$body_file")
if [ "$lines" -gt 40 ]; then
  fail "body has $lines lines, the limit is 40"
fi

[ "$failures" -eq 0 ]
