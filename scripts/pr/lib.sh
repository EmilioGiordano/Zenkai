#!/usr/bin/env bash
# Sourced by open.sh and card.sh. Project card helpers on top of gh and jq.

card_query='query($id:ID!){node(id:$id){... on ProjectV2Item{content{__typename ... on DraftIssue{id body}}}}}'
card_mutation='mutation($id:ID!,$body:String!){updateProjectV2DraftIssue(input:{draftIssueId:$id,body:$body}){draftIssue{id}}}'

# Usage: append_to_draft_card <dry-run 0|1> <project-item-id> <text>
append_to_draft_card() {
  local dry_run=$1 item=$2 text=$3
  if [ "$dry_run" -eq 1 ]; then
    echo "gh api graphql -f query='$card_query' -f id=$item"
    echo "gh api graphql -f query='$card_mutation' -f id=<draft-issue-id> -f body=\"<current body>\n\n$text\""
    return 0
  fi
  local content kind draft_id body
  content=$(gh api graphql -f query="$card_query" -f id="$item")
  kind=$(echo "$content" | jq -r '.data.node.content.__typename')
  if [ "$kind" != "DraftIssue" ]; then
    echo "card: item $item is a $kind, not a draft card" >&2
    return 1
  fi
  draft_id=$(echo "$content" | jq -r '.data.node.content.id')
  body=$(echo "$content" | jq -r '.data.node.content.body')
  gh api graphql -f query="$card_mutation" -f id="$draft_id" -f body="$body

$text" >/dev/null
}

# Usage: link_pr_to_card <dry-run 0|1> <project-item-id> <pr-url>
# A draft card gets a "PR: <url>" line; an issue card gets the PR added to project 2.
link_pr_to_card() {
  local dry_run=$1 item=$2 url=$3
  if [ "$dry_run" -eq 1 ]; then
    echo "# draft card:"
    append_to_draft_card 1 "$item" "PR: $url"
    echo "# issue card:"
    echo "gh project item-add 2 --owner EmilioGiordano --url $url"
    return 0
  fi
  local kind
  kind=$(gh api graphql -f query="$card_query" -f id="$item" | jq -r '.data.node.content.__typename')
  if [ "$kind" = "DraftIssue" ]; then
    append_to_draft_card 0 "$item" "PR: $url"
  else
    gh project item-add 2 --owner EmilioGiordano --url "$url"
  fi
}
