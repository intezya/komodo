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

require_count() {
  expected=$1
  needle=$2
  file=$3
  actual=$(grep -Fc "$needle" "$file" || true)
  if [ "$actual" -ne "$expected" ]; then
    echo "$file expected $expected occurrences of $needle, found $actual" >&2
    exit 1
  fi
}

if ! grep -Fq "workflow_dispatch:" "$release_workflow"; then
  echo "$release_workflow is missing workflow_dispatch rehearsal support" >&2
  exit 1
fi

for rehearsal_input in \
  "cache_mode:" \
  "cache_suffix:" \
  "dispatch_nonce:" \
  "expected_sha:"; do
  if ! grep -Fq "$rehearsal_input" "$release_workflow"; then
    echo "$release_workflow is missing $rehearsal_input" >&2
    exit 1
  fi
done

for rehearsal_guard in \
  "run-name:" \
  "group: release-cache" \
  "queue: max" \
  "cancel-in-progress: false" \
  'EXPECTED_SHA: ${{ inputs.expected_sha }}' \
  'DISPATCH_NONCE: ${{ inputs.dispatch_nonce }}' \
  'if [ "$GITHUB_REF" != "refs/heads/main" ]; then' \
  'if [ "$GITHUB_SHA" != "$EXPECTED_SHA" ]; then'; do
  if ! grep -Fq "$rehearsal_guard" "$release_workflow"; then
    echo "$release_workflow is missing rehearsal guard $rehearsal_guard" >&2
    exit 1
  fi
done

if grep -Fq "inputs.cache_mode == 'seed'" "$release_workflow" \
  || grep -Fxq "          - seed" "$release_workflow"; then
  echo "$release_workflow enables stable seeding before cargo-chef review" >&2
  exit 1
fi

require_count 2 "needs: preflight" "$release_workflow"
require_count 2 "push: \${{ github.event_name == 'push' }}" "$release_workflow"
require_count 2 "format('type=registry,ref=ghcr.io/intezya/komodo-build-cache:" "$release_workflow"
require_count 2 "format('type=registry,mode=max,image-manifest=true,oci-mediatypes=true,ref=ghcr.io/intezya/komodo-build-cache:" "$release_workflow"
require_count 3 "type=gha,scope=" "$release_workflow"
require_count 3 "type=gha,mode=max,scope=" "$release_workflow"

push_guard_count=$(grep -Fc "if: github.event_name == 'push'" "$release_workflow" || true)
if [ "$push_guard_count" -lt 4 ]; then
  echo "$release_workflow does not skip product/release work during rehearsal" >&2
  exit 1
fi

for dockerignore in \
  bin/binaries.Dockerfile.dockerignore \
  ui/Dockerfile.dockerignore \
  bin/core/single-arch.Dockerfile.dockerignore \
  bin/periphery/single-arch.Dockerfile.dockerignore \
  bin/cli/single-arch.Dockerfile.dockerignore; do
  if [ ! -f "$dockerignore" ] || ! grep -Fxq '*' "$dockerignore"; then
    echo "$dockerignore must remain an effective strict allowlist" >&2
    exit 1
  fi
done

for ignored in '**/.env' '**/.env.*' '**/node_modules' '**/dist' '**/*.pem' '**/*.key'; do
  if ! grep -Fxq "$ignored" ui/Dockerfile.dockerignore; then
    echo "ui/Dockerfile.dockerignore is missing $ignored" >&2
    exit 1
  fi
done

if grep -Eq '^!(([^/]*/)*\.git(/.*)?|([^/]*/)*\.env(\..*|\*)?|.*\.(pem|key))$' \
  bin/binaries.Dockerfile.dockerignore \
  ui/Dockerfile.dockerignore \
  bin/core/single-arch.Dockerfile.dockerignore \
  bin/periphery/single-arch.Dockerfile.dockerignore \
  bin/cli/single-arch.Dockerfile.dockerignore; then
  echo "an effective Dockerfile ignore file re-includes secret-bearing paths" >&2
  exit 1
fi

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
