#!/usr/bin/env sh
set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: $0 OUTPUT_JSON" >&2
  exit 2
fi

output=$1
owner=intezya
repo=intezya/komodo
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

gh api --paginate \
  "/users/$owner/packages?package_type=container&per_page=100" \
  > "$tmp/package-pages.json"
jq -s 'add' "$tmp/package-pages.json" > "$tmp/packages.json"
: > "$tmp/product-rows.jsonl"

for package in \
  komodo-binaries komodo-ui komodo-core komodo-periphery komodo-cli; do
  if jq -e --arg package "$package" \
    'any(.[]; .name == $package)' "$tmp/packages.json" >/dev/null; then
    gh api --paginate \
      "/users/$owner/packages/container/$package/versions?per_page=100" \
      > "$tmp/version-pages.json"
    jq -cs --arg package "$package" \
      'add | {package:$package,absent:false,versions:(map({id,created_at,updated_at,tags:.metadata.container.tags}) | sort_by(.id))}' \
      "$tmp/version-pages.json" >> "$tmp/product-rows.jsonl"
  else
    jq -cn --arg package "$package" \
      '{package:$package,absent:true,versions:[]}' \
      >> "$tmp/product-rows.jsonl"
  fi
done

gh api --paginate "repos/$repo/releases?per_page=100" \
  > "$tmp/release-pages.json"
jq -cs --slurpfile packages "$tmp/product-rows.jsonl" \
  '{packages:($packages | sort_by(.package)),releases:(add | map({id,tag_name,created_at}) | sort_by(.id))}' \
  "$tmp/release-pages.json" > "$tmp/output.json"
jq -e '.packages | length == 5' "$tmp/output.json" >/dev/null
mv "$tmp/output.json" "$output"
