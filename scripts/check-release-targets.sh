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
  bin/core/single-arch.Dockerfile \
  bin/periphery/single-arch.Dockerfile \
  bin/cli/single-arch.Dockerfile \
  ui/Dockerfile; then
  echo "release-critical files still reference moghtech" >&2
  exit 1
fi

if grep -q "aio.Dockerfile" "$release_workflow"; then
  echo "$release_workflow still rebuilds release images from aio Dockerfiles" >&2
  exit 1
fi

for image in komodo-binaries komodo-ui; do
  if ! grep -q "$image" "$release_workflow"; then
    echo "$release_workflow does not publish $image" >&2
    exit 1
  fi
done

if ! grep -Fq 'docker create --entrypoint /km "ghcr.io/intezya/komodo-binaries:${GITHUB_REF_NAME}"' "$release_workflow"; then
  echo "$release_workflow does not create the binaries container with an explicit command" >&2
  exit 1
fi

for dockerfile in \
  bin/binaries.Dockerfile \
  ui/Dockerfile \
  bin/core/single-arch.Dockerfile \
  bin/periphery/single-arch.Dockerfile \
  bin/cli/single-arch.Dockerfile; do
  if ! grep -q "$dockerfile" "$release_workflow"; then
    echo "$release_workflow does not use $dockerfile" >&2
    exit 1
  fi

  if [ ! -f "$dockerfile.dockerignore" ]; then
    echo "$dockerfile is missing a Dockerfile-specific ignore file" >&2
    exit 1
  fi
done

for cache_directive in "cache-from: type=gha" "cache-to: type=gha,mode=max"; do
  if ! grep -q "$cache_directive" "$release_workflow"; then
    echo "$release_workflow is missing $cache_directive" >&2
    exit 1
  fi
done

if grep -q "cargo build -p komodo_core --release &&" bin/binaries.Dockerfile; then
  echo "bin/binaries.Dockerfile still builds release packages sequentially" >&2
  exit 1
fi

if grep -q "cargo install cargo-strip" bin/binaries.Dockerfile; then
  echo "bin/binaries.Dockerfile still installs cargo-strip during release builds" >&2
  exit 1
fi

for cache_mount in \
  "/usr/local/cargo/registry" \
  "/usr/local/cargo/git" \
  "/builder/target"; do
  if ! grep -q "type=cache,target=$cache_mount" bin/binaries.Dockerfile; then
    echo "bin/binaries.Dockerfile is missing cache mount $cache_mount" >&2
    exit 1
  fi
done

echo "release targets point at intezya and reuse built artifacts"
