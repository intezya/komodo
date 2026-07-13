#!/usr/bin/env sh
set -eu

if [ "$#" -lt 6 ]; then
  echo "usage: $0 OWNER/REPO WORKFLOW REF EXPECTED_TITLE EXPECTED_SHA dispatch-args..." >&2
  exit 2
fi

repo=$1
workflow=$2
ref=$3
expected_title=$4
expected_sha=$5
shift 5

case "$expected_sha" in
  ''|*[!0-9a-f]* )
    echo "EXPECTED_SHA must be lowercase hexadecimal" >&2
    exit 2
    ;;
esac
if [ "${#expected_sha}" -ne 40 ]; then
  echo "EXPECTED_SHA must contain exactly 40 characters" >&2
  exit 2
fi

endpoint="repos/$repo/actions/workflows/$workflow/runs?event=workflow_dispatch&branch=$ref&per_page=100"
before=$(gh api "$endpoint" | jq '[.workflow_runs[].id] | max // 0')
actor=$(gh api user --jq .login)

gh workflow run "$workflow" --repo "$repo" --ref "$ref" "$@"

attempt=0
while [ "$attempt" -lt 60 ]; do
  candidates=$(gh api "$endpoint" | jq -c \
    --argjson before "$before" \
    --arg actor "$actor" \
    --arg title "$expected_title" \
    '[.workflow_runs[] | select(
      .id > $before and
      .actor.login == $actor and
      .display_title == $title
    )] | sort_by(.id)')
  count=$(printf '%s' "$candidates" | jq 'length')
  if [ "$count" -gt 1 ]; then
    echo "ambiguous dispatch: $count exact-title runs appeared" >&2
    exit 1
  fi
  if [ "$count" -eq 1 ]; then
    id=$(printf '%s' "$candidates" | jq -r '.[0].id')
    selected_sha=$(printf '%s' "$candidates" | jq -r '.[0].head_sha')
    if [ "$selected_sha" != "$expected_sha" ]; then
      gh run cancel --repo "$repo" "$id" >/dev/null 2>&1 || true
      echo "cancelled run $id: selected SHA $selected_sha does not match expected $expected_sha" >&2
      exit 1
    fi
    printf '%s\n' "$id"
    exit 0
  fi
  attempt=$((attempt + 1))
  sleep 2
done

echo "timed out waiting for the dispatched $workflow run" >&2
exit 1
