#!/usr/bin/env bash
# Sourced by open.sh and card.sh. Project card helpers on top of gh and jq.

project_number=${ZENKAI_PROJECT_NUMBER:-2}
project_owner=${ZENKAI_PROJECT_OWNER:-EmilioGiordano}
card_query='query($id:ID!){node(id:$id){... on ProjectV2Item{content{__typename ... on DraftIssue{id body} ... on Issue{number repository{nameWithOwner}}}}}}'
card_mutation='mutation($id:ID!,$body:String!){updateProjectV2DraftIssue(input:{draftIssueId:$id,body:$body}){draftIssue{id}}}'

# Usage: card_content <project-item-id>; prints the content JSON, fails when the item has none.
card_content() {
  local response content kind
  response=$(gh api graphql -f query="$card_query" -f id="$1") || return 1
  content=$(echo "$response" | jq -c '.data.node.content // empty') || return 1
  kind=$(echo "$content" | jq -r '.__typename // empty') || return 1
  if [ -z "$kind" ]; then
    echo "card: item $1 has no content" >&2
    return 1
  fi
  echo "$content"
}

# Usage: append_to_draft_card <dry-run 0|1> <project-item-id> <text>
append_to_draft_card() {
  local dry_run=$1 item=$2 text=$3
  if [ "$dry_run" -eq 1 ]; then
    echo "gh api graphql -f query='$card_query' -f id=$item"
    echo "gh api graphql -f query='$card_mutation' -f id=<draft-issue-id> -f body=\"<current body>\n\n$text\""
    return 0
  fi
  local content kind draft_id body
  content=$(card_content "$item") || return 1
  kind=$(echo "$content" | jq -r '.__typename')
  if [ "$kind" != "DraftIssue" ]; then
    echo "card: item $item is a $kind, not a draft card" >&2
    return 1
  fi
  draft_id=$(echo "$content" | jq -r '.id // empty')
  if [ -z "$draft_id" ]; then
    echo "card: item $item has no draft issue id" >&2
    return 1
  fi
  body=$(echo "$content" | jq -r '.body // ""')
  gh api graphql -f query="$card_mutation" -f id="$draft_id" -f body="$body

$text" >/dev/null
}

# Usage: add_tldr_to_card <dry-run 0|1> <project-item-id> <text>
# A draft card gets the text appended; an issue card gets it as a comment on the issue.
add_tldr_to_card() {
  local dry_run=$1 item=$2 text=$3
  if [ "$dry_run" -eq 1 ]; then
    echo "# draft card:"
    append_to_draft_card 1 "$item" "$text"
    echo "# issue card:"
    echo "gh issue comment <number> --repo <owner/repo> --body \"$text\""
    return 0
  fi
  local content kind
  content=$(card_content "$item") || return 1
  kind=$(echo "$content" | jq -r '.__typename')
  case "$kind" in
    DraftIssue) append_to_draft_card 0 "$item" "$text" ;;
    Issue)
      gh issue comment "$(echo "$content" | jq -r '.number')" \
        --repo "$(echo "$content" | jq -r '.repository.nameWithOwner')" --body "$text" >/dev/null
      ;;
    *)
      echo "card: item $item is a $kind; only draft and issue cards take a TLDR" >&2
      return 1
      ;;
  esac
}

# Usage: link_pr_to_card <dry-run 0|1> <project-item-id> <pr-url>
# A draft card gets a "PR: <url>" line; an issue card gets the PR added to the project.
link_pr_to_card() {
  local dry_run=$1 item=$2 url=$3
  if [ "$dry_run" -eq 1 ]; then
    echo "# draft card:"
    append_to_draft_card 1 "$item" "PR: $url"
    echo "# issue card:"
    echo "gh project item-add $project_number --owner $project_owner --url $url"
    return 0
  fi
  local content kind
  content=$(card_content "$item") || return 1
  kind=$(echo "$content" | jq -r '.__typename')
  if [ "$kind" = "DraftIssue" ]; then
    append_to_draft_card 0 "$item" "PR: $url"
  else
    gh project item-add "$project_number" --owner "$project_owner" --url "$url"
  fi
}
