#!/usr/bin/env sh
set -eu

if [ "$#" -lt 2 ]; then
  echo "usage: $0 OUTPUT_JSON TAG... | OUTPUT_JSON --all-version-ids" >&2
  exit 2
fi

output=$1
shift
owner=intezya
package=komodo-build-cache
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

gh api --paginate \
  "/users/$owner/packages?package_type=container&per_page=100" \
  > "$tmp/package-pages.json"
jq -s 'add' "$tmp/package-pages.json" > "$tmp/packages.json"
: > "$tmp/cache-rows.jsonl"

package_exists=false
if jq -e --arg package "$package" \
  'any(.[]; .name == $package)' "$tmp/packages.json" >/dev/null; then
  package_exists=true
  gh api --paginate \
    "/users/$owner/packages/container/$package/versions?per_page=100" \
    > "$tmp/version-pages.json"
  jq -s 'add' "$tmp/version-pages.json" > "$tmp/versions.json"
fi

if [ "$#" -eq 1 ] && [ "$1" = --all-version-ids ]; then
  if [ "$package_exists" = false ]; then
    printf '[]\n' > "$tmp/output.json"
  else
    jq '[.[] | {id,created_at,updated_at,tags:.metadata.container.tags}] | sort_by(.id)' \
      "$tmp/versions.json" > "$tmp/output.json"
  fi
  jq -e '([.[].id] | length) == ([.[].id] | unique | length)' \
    "$tmp/output.json" >/dev/null
  mv "$tmp/output.json" "$output"
  exit 0
fi

for tag do
  if [ "$package_exists" = false ]; then
    jq -cn --arg tag "$tag" '{tag:$tag,absent:true}' \
      >> "$tmp/cache-rows.jsonl"
    continue
  fi
  version=$(jq -c --arg tag "$tag" \
    '[.[] | select(any(.metadata.container.tags[]?; . == $tag))]' \
    "$tmp/versions.json")
  count=$(printf '%s' "$version" | jq 'length')
  if [ "$count" -eq 0 ]; then
    jq -cn --arg tag "$tag" '{tag:$tag,absent:true}' \
      >> "$tmp/cache-rows.jsonl"
    continue
  fi
  if [ "$count" -ne 1 ]; then
    echo "cache tag $tag resolves to $count package versions" >&2
    exit 1
  fi
  docker buildx imagetools inspect --raw \
    "ghcr.io/$owner/$package:$tag" > "$tmp/manifest.json"
  layer_bytes=$(jq -e \
    'if (.layers | type) != "array" or (.layers | length) == 0 then error("expected non-empty OCI layers") else ([.layers[].size] | add) end' \
    "$tmp/manifest.json")
  manifest_bytes=$(wc -c < "$tmp/manifest.json" | tr -d ' ')
  jq -cn --arg tag "$tag" --argjson version "$version" \
    --argjson layer_bytes "$layer_bytes" \
    --argjson manifest_bytes "$manifest_bytes" \
    '{tag:$tag,absent:false,id:$version[0].id,updated_at:$version[0].updated_at,tags:$version[0].metadata.container.tags,layer_bytes:$layer_bytes,manifest_bytes:$manifest_bytes}' \
    >> "$tmp/cache-rows.jsonl"
done

jq -s 'sort_by(.tag)' "$tmp/cache-rows.jsonl" > "$tmp/output.json"
jq -e --argjson expected "$#" 'length == $expected' \
  "$tmp/output.json" >/dev/null
mv "$tmp/output.json" "$output"
