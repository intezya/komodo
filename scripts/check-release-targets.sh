#!/usr/bin/env sh
set -eu

release_workflow=".github/workflows/release.yml"

if [ ! -f "$release_workflow" ]; then
  echo "missing $release_workflow" >&2
  exit 1
fi

if ! grep -q "intezya" "$release_workflow"; then
  echo "$release_workflow does not target intezya" >&2
  exit 1
fi

if grep -q "moghtech" \
  "$release_workflow" \
  scripts/install-cli.py \
  scripts/setup-periphery.py \
  scripts/readme.md \
  compose/*.compose.yaml \
  bin/binaries.Dockerfile \
  bin/core/aio.Dockerfile \
  bin/periphery/aio.Dockerfile \
  bin/cli/aio.Dockerfile; then
  echo "release-critical files still reference moghtech" >&2
  exit 1
fi

echo "release targets point at intezya"
