# Komodo Build, Release, and Dependency Performance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make trusted CI and releases reuse Rust, UI, and BuildKit work across runs and distinct release tags while preserving cold-build reproducibility and cache trust boundaries.

**Architecture:** Replace tag-scoped GHA BuildKit exports with stable GHCR registry-cache references written only by an explicit trusted-`main` seed workflow; release tags are read-only cache consumers. When product packages are initially absent, seed uses two unique temporary images inside `komodo-build-cache` as matrix bases and deletes their isolated versions before completion. Split Rust dependency compilation from source compilation with pinned `cargo-chef`, restore compiler-aware CI caching, and close the dependency finding with an executable graph audit. The audit retains `mogh_config`'s Cicada feature because arbitrary `cicada:` config paths are a supported runtime input, so removing it would be a compatibility change rather than a safe optimization.

**Tech Stack:** GitHub Actions, Docker Buildx/BuildKit, GHCR OCI cache manifests, Rust 1.95, Cargo workspace, cargo-chef 0.1.77, Swatinem/rust-cache 2.9.1, POSIX shell.

---

## Scope and checkpoint map

This umbrella plan is implemented as four ordered, independently reviewable PR checkpoints:

1. **Non-publishing release-cache rehearsal** — Tasks 1–3. Tag releases keep
   their existing GHA cache source throughout this checkpoint.
2. **Cargo dependency layers and stable registry scopes** — Tasks 4–6. This
   checkpoint is the first point where tag releases read stable GHCR refs and
   trusted `main` may seed them.
3. **CI Rust cache and cancellation** — Tasks 7–9.
4. **Dependency graph decision and final delivery proof** — Tasks 10–12.

The first three checkpoints change delivery behavior but not application
semantics. Checkpoint 4 records why the apparent Cicada dependency is retained
and verifies there are no other removable workspace edges in this P1; it does
not silently change configuration semantics.

## File ownership map

- `.github/workflows/release.yml` — trusted release and non-publishing cache-rehearsal orchestration.
- `.github/workflows/ci.yml` — Rust CI cache policy, manual measurement input, and run cancellation.
- `bin/binaries.Dockerfile.dockerignore`, `ui/Dockerfile.dockerignore`, and
  `bin/{core,periphery,cli}/single-arch.Dockerfile.dockerignore` — the effective
  Dockerfile-specific build-context allowlists. These take precedence over a
  repository-root `.dockerignore` and therefore own release cache safety.
- `bin/binaries.Dockerfile` — pinned cargo-chef planner/cook/application stages.
- `scripts/check-release-targets.sh` — executable regression contract for release workflow, cache scopes, and Dockerfile stages.
- `scripts/check-ci-cache.sh` — executable regression contract for CI cache trust and cancellation.
- `scripts/dispatch-workflow-run.sh` — dispatch one manual workflow and return
  only the newly created run that matches trusted actor, expected SHA, and
  exact title; cancel a mismatched dispatch before dependent jobs can run.
- `scripts/snapshot-release-state.sh` — fail-closed product-package and GitHub
  release inventory, including explicit absent-package records.
- `scripts/inspect-registry-cache.sh` — fail-closed cache-tag inventory with OCI
  compressed layer-byte totals rather than raw manifest JSON length.
- `scripts/probe-cargo-chef.sh` — one isolated Buildx builder shared by cold,
  source-only, manifest-change, and lockfile-change probes.
- `scripts/delete-seed-bootstrap.sh` — fail-closed deletion of the two unique,
  single-tag cache-package images used to bootstrap an initially empty fork.
- `ui/Dockerfile` and `bin/{core,periphery,cli}/single-arch.Dockerfile` — final image revision labels and cold-start smoke targets.
- `docs/performance/build-release-dependency-audit.md` — measured cache results and the no-change Cicada compatibility decision.

## References

- cargo-chef usage and same-toolchain requirement: <https://github.com/LukeMathWalker/cargo-chef#how-to-use>
- Docker registry cache backend: <https://docs.docker.com/build/cache/backends/registry/>
- GitHub cache trust and branch restrictions: <https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching>
- GitHub concurrency queue semantics: <https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/control-workflow-concurrency>
- Rust cache inputs and `save-if`: <https://github.com/Swatinem/rust-cache#example-usage>

### Task 1: Turn release-cache expectations into a failing executable contract

**Files:**
- Modify: `scripts/check-release-targets.sh:60-85`
- Test: `scripts/check-release-targets.sh`

- [ ] **Step 1: Replace the GHA-cache assertions with registry-cache and rehearsal assertions**

Replace the current `cache_directive` loop with this complete transitional
checkpoint contract. It deliberately requires tag builds to retain their
existing GHA cache while only manual rehearsal builds use disposable registry
refs:

```sh
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
```

- [ ] **Step 2: Run the contract and verify the current workflow fails**

Run:

```bash
rtk sh scripts/check-release-targets.sh
```

Expected: FAIL with `release.yml is missing workflow_dispatch rehearsal support`.

- [ ] **Step 3: Check the shell syntax independently**

Run:

```bash
rtk sh -n scripts/check-release-targets.sh
```

Expected: exit 0 with no output.

### Task 2: Add a non-publishing rehearsal without changing tag cache sources

**Files:**
- Modify: `ui/Dockerfile.dockerignore`
- Verify: `bin/binaries.Dockerfile.dockerignore`
- Verify: `bin/{core,periphery,cli}/single-arch.Dockerfile.dockerignore`
- Modify: `.github/workflows/release.yml:1-156`
- Create: `scripts/dispatch-workflow-run.sh`
- Create: `scripts/snapshot-release-state.sh`
- Create: `scripts/inspect-registry-cache.sh`
- Test: `scripts/check-release-targets.sh`

- [ ] **Step 1: Add the guarded manual trigger**

Change the trigger to:

```yaml
on:
  push:
    tags:
      - "v*"
  workflow_dispatch:
    inputs:
      cache_mode:
        description: "Write a disposable rehearsal cache"
        required: true
        default: rehearsal
        type: choice
        options:
          - rehearsal
      cache_suffix:
        description: "Disposable suffix, for example -rehearsal-p1-cache"
        required: false
        default: "-rehearsal-manual"
        type: string
      dispatch_nonce:
        description: "Unique a-z0-9-hyphen selector for this dispatch"
        required: true
        type: string
      expected_sha:
        description: "Exact 40-character main SHA expected by this dispatch"
        required: true
        type: string

run-name: >-
  ${{ github.event_name == 'workflow_dispatch' && format('Release dispatch={0} cache={1} suffix={2} sha={3}', inputs.dispatch_nonce, inputs.cache_mode, inputs.cache_suffix, inputs.expected_sha) || format('Release tag={0}', github.ref_name) }}

concurrency:
  group: release-cache
  queue: max
  cancel-in-progress: false
```

One constant group serializes tag releases and every manual cache writer;
`queue: max` retains up to GitHub's documented 100 pending runs instead of
replacing the previous pending release when a third run arrives. This
prevents two distinct rehearsal suffixes or a seed from interleaving package
versions, and makes full-version-ID cleanup attribution fail closed.

- [ ] **Step 2: Add a preflight job that accepts only rehearsal refs on trusted main**

Insert this job before `binaries`:

```yaml
  preflight:
    name: Validate release mode
    runs-on: ubuntu-latest
    steps:
      - name: Validate rehearsal inputs
        if: github.event_name == 'workflow_dispatch'
        env:
          CACHE_MODE: ${{ inputs.cache_mode }}
          CACHE_SUFFIX: ${{ inputs.cache_suffix }}
          DISPATCH_NONCE: ${{ inputs.dispatch_nonce }}
          EXPECTED_SHA: ${{ inputs.expected_sha }}
        run: |
          if [ "$GITHUB_REF" != "refs/heads/main" ]; then
            echo "manual cache jobs must run from main" >&2
            exit 1
          fi
          case "$EXPECTED_SHA" in
            ''|*[!0-9a-f]* )
              echo "expected_sha must contain only lowercase hexadecimal" >&2
              exit 1
              ;;
          esac
          if [ "${#EXPECTED_SHA}" -ne 40 ]; then
            echo "expected_sha must contain exactly 40 characters" >&2
            exit 1
          fi
          if [ "$GITHUB_SHA" != "$EXPECTED_SHA" ]; then
            echo "dispatch resolved $GITHUB_SHA instead of $EXPECTED_SHA" >&2
            exit 1
          fi
          case "$DISPATCH_NONCE" in
            ''|*[!a-z0-9-]* )
              echo "dispatch_nonce must be non-empty and use only a-z, 0-9, and hyphen" >&2
              exit 1
              ;;
          esac
          case "$DISPATCH_NONCE" in
            *[a-z0-9]* ) ;;
            *) echo "dispatch_nonce must contain a letter or digit" >&2; exit 1 ;;
          esac
          if [ "$CACHE_MODE" != rehearsal ]; then
            echo "checkpoint 1 only permits rehearsal mode" >&2
            exit 1
          fi
          body=${CACHE_SUFFIX#-rehearsal-}
          if [ "$body" = "$CACHE_SUFFIX" ] || [ -z "$body" ]; then
            echo "rehearsal cache_suffix must start with -rehearsal- and have a non-empty body" >&2
            exit 1
          fi
          case "$body" in
            *[!a-z0-9-]* )
              echo "rehearsal cache_suffix body must use only a-z, 0-9, and hyphen" >&2
              exit 1
              ;;
            *[a-z0-9]* ) ;;
            *) echo "rehearsal cache_suffix body must contain a letter or digit" >&2; exit 1 ;;
          esac
```

Add `needs: preflight` to both the `binaries` and `ui` jobs.

- [ ] **Step 3: Harden the effective Dockerfile-specific contexts**

Do not create a repository-root `.dockerignore` for this checkpoint. Every
release Dockerfile already has a `<Dockerfile>.dockerignore`, and Docker uses
that file instead of the root file. Keep the existing allowlist in
`bin/binaries.Dockerfile.dockerignore`; it starts with `*` and re-includes only
the Cargo manifests, `.cargo`, the real Rust workspace members, and `xtask`.

Append the recursive secret and generated-output exclusions below to the end
of `ui/Dockerfile.dockerignore`, after its `!ui/**` and
`!client/core/ts/**` exceptions:

```dockerignore
**/.env
**/.env.*
**/node_modules
**/dist
**/*.pem
**/*.key
```

The Core, Periphery, and CLI final-image ignore files remain strict allowlists;
CLI intentionally contains only `*` because it copies no local context files.
Run `rtk git status --ignored --short` to review ignored local filenames without
opening their contents. The contracts check the effective files and reject any
future `!.git`, `!.env`, `!**/.env*`, `!*.pem`, or `!*.key` exception.

- [ ] **Step 4: Add an exact workflow-dispatch run selector**

Create `scripts/dispatch-workflow-run.sh`:

```sh
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
```

Run `rtk sh -n scripts/dispatch-workflow-run.sh`. Expected: exit 0. Every
later dispatch in this plan must fetch one expected SHA, pass it both as the
helper's fifth positional argument and as `-f expected_sha="$expected_sha"`,
and use this helper; never select `--limit 1`. If `main` moves between SHA
capture and dispatch, workflow preflight fails before cache jobs and the helper
cancels the exact mismatched run.

- [ ] **Step 5: Add fail-closed product and cache inventory helpers**

Create `scripts/snapshot-release-state.sh` using plain portable commands. The
helper handles an absent product package as data, but any unexpected API or
JSON error aborts before replacing the requested output:

```sh
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
```

Create `scripts/inspect-registry-cache.sh`. Registry cache exports in this plan
set `image-manifest=true,oci-mediatypes=true`, so every present ref must expose
an OCI `layers` array. The helper sums compressed layer descriptor sizes and
fails if inspection returns an index, malformed JSON, or zero layers:

```sh
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
```

Run `rtk sh -n` on both helpers. They intentionally contain no `rtk`; callers
are responsible for wrapping execution.

- [ ] **Step 6: Give manual binaries builds disposable registry cache while preserving tag GHA cache**

Use these values in `Publish binaries image`:

```yaml
          push: ${{ github.event_name == 'push' }}
          tags: |
            ghcr.io/intezya/komodo-binaries:${{ github.ref_name }}
            ghcr.io/intezya/komodo-binaries:2
          cache-from: ${{ github.event_name == 'workflow_dispatch' && format('type=registry,ref=ghcr.io/intezya/komodo-build-cache:binaries{0}', inputs.cache_suffix) || 'type=gha,scope=komodo-binaries' }}
          cache-to: ${{ github.event_name == 'workflow_dispatch' && format('type=registry,mode=max,image-manifest=true,oci-mediatypes=true,ref=ghcr.io/intezya/komodo-build-cache:binaries{0}', inputs.cache_suffix) || 'type=gha,mode=max,scope=komodo-binaries' }}
          build-args: |
            SOURCE_REVISION=${{ github.sha }}
```

Add the same condition to the following binaries-only release steps:

```yaml
      - name: Prepare release assets
        if: github.event_name == 'push'
        run: |
          mkdir -p dist/assets
          docker pull "ghcr.io/intezya/komodo-binaries:${GITHUB_REF_NAME}"
          container_id="$(docker create "ghcr.io/intezya/komodo-binaries:${GITHUB_REF_NAME}")"
          docker cp "$container_id:/km" dist/assets/km-x86_64
          docker cp "$container_id:/periphery" dist/assets/periphery-x86_64
          docker cp "$container_id:/core" dist/assets/core-x86_64
          docker rm "$container_id"
          chmod +x dist/assets/*

      - name: Upload release assets
        if: github.event_name == 'push'
        uses: actions/upload-artifact@v4
        with:
          name: linux-amd64-release-assets
          path: dist/assets/*
          if-no-files-found: error
```

- [ ] **Step 7: Apply the same transitional cache policy to UI**

Use:

```yaml
          push: ${{ github.event_name == 'push' }}
          tags: |
            ghcr.io/intezya/komodo-ui:${{ github.ref_name }}
            ghcr.io/intezya/komodo-ui:2
          build-args: |
            SOURCE_REVISION=${{ github.sha }}
          cache-from: ${{ github.event_name == 'workflow_dispatch' && format('type=registry,ref=ghcr.io/intezya/komodo-build-cache:ui{0}', inputs.cache_suffix) || 'type=gha,scope=komodo-ui' }}
          cache-to: ${{ github.event_name == 'workflow_dispatch' && format('type=registry,mode=max,image-manifest=true,oci-mediatypes=true,ref=ghcr.io/intezya/komodo-build-cache:ui{0}', inputs.cache_suffix) || 'type=gha,mode=max,scope=komodo-ui' }}
```

- [ ] **Step 8: Skip product-image and GitHub-release jobs during rehearsal**

Add the job condition:

```yaml
  images:
    if: github.event_name == 'push'
```

Add a release-only condition to the GitHub release job:

```yaml
  release:
    if: github.event_name == 'push'
```

At the end of checkpoint 1, tag runs still read and write their original GHA
scopes. Only the two manual binaries/UI steps touch GHCR, and only under the
validated disposable suffix. Stable registry refs and seed mode do not exist
until Task 5.

- [ ] **Step 9: Run the release contract**

Run:

```bash
rtk sh scripts/check-release-targets.sh
```

Expected: PASS with `release targets point at intezya and reuse built artifacts`.

- [ ] **Step 10: Verify the diff is syntactically clean**

Run:

```bash
rtk actionlint -ignore 'unexpected key "queue"' .github/workflows/release.yml
rtk git diff --check
```

Expected: both commands exit 0 with no output. The narrow ignore exists because
`actionlint v1.7.12` has not yet added GitHub's supported `concurrency.queue`
schema key; `check-release-targets.sh` still requires the exact `queue: max`
policy, so no other workflow diagnostic is hidden.

- [ ] **Step 11: Commit checkpoint 1 implementation**

```bash
rtk git add .github/workflows/release.yml ui/Dockerfile.dockerignore scripts/check-release-targets.sh scripts/dispatch-workflow-run.sh scripts/snapshot-release-state.sh scripts/inspect-registry-cache.sh
rtk git commit -m "ci: add safe release cache rehearsal"
```

### Task 3: Prove registry cache reuse without publishing product images

**Files:**
- Verify: `.github/workflows/release.yml`
- Verify: GHCR package `intezya/komodo-build-cache`
- Verify: `scripts/snapshot-release-state.sh`
- Verify: `scripts/inspect-registry-cache.sh`

This task runs after the Task 2 checkpoint has merged to `main`; `workflow_dispatch` definitions are read from the default branch.

- [ ] **Step 1: Satisfy the hard package-auth prerequisite**

Before starting an expensive rehearsal, verify that the active `gh` token can
read and delete package versions and that Docker can authenticate to GHCR:

```bash
rtk gh api -i user 2>&1 | rtk rg -qi '^x-oauth-scopes:.*(read|write):packages'
rtk gh api -i user 2>&1 | rtk rg -qi '^x-oauth-scopes:.*delete:packages'
rtk gh auth token | rtk docker login ghcr.io --username intezya --password-stdin
```

Expected: all three commands succeed. `delete:packages` is a hard prerequisite,
not an optional cleanup improvement. If it is missing, stop here and ask the
operator to authorize `rtk gh auth refresh -h github.com -s read:packages -s
delete:packages`; do not run rehearsals that cannot be cleaned up.

- [ ] **Step 2: Create one unique absent-checked rehearsal batch**

Run:

```bash
rtk proxy sh -c 'set -eu; rtk mkdir -p target; batch="p1-cache-$(rtk date +%s)-$$"; suffix="-rehearsal-$batch"; expected_sha=$(rtk gh api repos/intezya/komodo/commits/main --jq .sha); rtk printf "%s\n" "$suffix" > target/release-rehearsal-suffix.txt; rtk printf "%s\n" "$expected_sha" > target/release-rehearsal-sha.txt; rtk sh scripts/snapshot-release-state.sh target/release-products-before.json; rtk sh scripts/inspect-registry-cache.sh target/release-cache-before.json "binaries$suffix" "ui$suffix"; rtk jq -e "length == 2 and all(.[]; .absent == true)" target/release-cache-before.json; rtk sh scripts/inspect-registry-cache.sh target/release-cache-versions-before.json --all-version-ids'
```

Expected: the product/release snapshot succeeds even when a named package is
absent, both unique cache refs are explicitly absent, and one immutable expected
SHA plus one suffix are saved for the whole pair.

- [ ] **Step 3: Run the first disposable rehearsal**

Run:

```bash
rtk proxy sh -c 'set -eu; suffix=$(rtk sed -n "1p" target/release-rehearsal-suffix.txt); expected_sha=$(rtk sed -n "1p" target/release-rehearsal-sha.txt); nonce="p1-cache-first-$(rtk date +%s)-$$"; title="Release dispatch=$nonce cache=rehearsal suffix=$suffix sha=$expected_sha"; id=$(rtk sh scripts/dispatch-workflow-run.sh intezya/komodo release.yml main "$title" "$expected_sha" -f cache_mode=rehearsal -f cache_suffix="$suffix" -f dispatch_nonce="$nonce" -f expected_sha="$expected_sha"); rtk printf "%s\n" "$id" > target/release-rehearsal-first-id.txt; rtk gh run watch --repo intezya/komodo "$id" --exit-status; rtk proxy gh run view --repo intezya/komodo "$id" --log > target/release-rehearsal-first.log'
```

Expected: the exact newly dispatched run ID is printed and succeeds; only
`preflight`, `binaries`, and `ui` run. The complete run log is saved for cache
evidence.

- [ ] **Step 4: Verify both new refs after the first rehearsal**

Run:

```bash
rtk proxy sh -c 'set -eu; suffix=$(rtk sed -n "1p" target/release-rehearsal-suffix.txt); rtk sh scripts/inspect-registry-cache.sh target/release-cache-after-first.json "binaries$suffix" "ui$suffix"; rtk jq -e "length == 2 and all(.[]; .absent == false and .layer_bytes > 0)" target/release-cache-after-first.json; rtk sh scripts/inspect-registry-cache.sh target/release-cache-versions-after-first.json --all-version-ids'
```

Expected: both disposable cache scopes are exported. No `komodo-binaries`,
`komodo-ui`, final image, or GitHub release is published.

- [ ] **Step 5: Run the second rehearsal with the same suffix and SHA**

Run:

```bash
rtk proxy sh -c 'set -eu; suffix=$(rtk sed -n "1p" target/release-rehearsal-suffix.txt); expected_sha=$(rtk sed -n "1p" target/release-rehearsal-sha.txt); nonce="p1-cache-second-$(rtk date +%s)-$$"; title="Release dispatch=$nonce cache=rehearsal suffix=$suffix sha=$expected_sha"; id=$(rtk sh scripts/dispatch-workflow-run.sh intezya/komodo release.yml main "$title" "$expected_sha" -f cache_mode=rehearsal -f cache_suffix="$suffix" -f dispatch_nonce="$nonce" -f expected_sha="$expected_sha"); rtk printf "%s\n" "$id" > target/release-rehearsal-second-id.txt; rtk gh run watch --repo intezya/komodo "$id" --exit-status; rtk proxy gh run view --repo intezya/komodo "$id" --log > target/release-rehearsal-second.log; for scope in binaries ui; do job_name="Build and publish $scope"; job_id=$(rtk gh run view --repo intezya/komodo "$id" --json jobs --jq "[.jobs[] | select(.name == \"$job_name\")] | if length == 1 then .[0].databaseId else error(\"expected one job\") end"); rtk proxy gh run view --repo intezya/komodo --job "$job_id" --log > "target/release-rehearsal-second-$scope.log"; rtk rg -F "importing cache manifest from ghcr.io/intezya/komodo-build-cache:$scope$suffix" "target/release-rehearsal-second-$scope.log"; rtk rg -n "CACHED" "target/release-rehearsal-second-$scope.log"; done'
```

Expected: the second exact run succeeds, both jobs import their disposable
registry manifests, and the downloaded log contains cached build steps.

- [ ] **Step 6: Prove no product package or release changed**

```bash
rtk sh scripts/snapshot-release-state.sh target/release-products-after.json
rtk cmp target/release-products-before.json target/release-products-after.json
```

Expected: both comparisons exit 0. Rehearsal created only the two disposable
cache refs.

- [ ] **Step 7: Record exact run proof**

```bash
rtk proxy sh -c 'set -eu; for file in target/release-rehearsal-first-id.txt target/release-rehearsal-second-id.txt; do id=$(rtk sed -n "1p" "$file"); rtk gh run view --repo intezya/komodo "$id" --json databaseId,headSha,conclusion,startedAt,updatedAt,url; done' | rtk proxy tee target/release-rehearsal-runs.jsonl
rtk proxy sh -c 'set -eu; expected_sha=$(rtk sed -n "1p" target/release-rehearsal-sha.txt); rtk jq -se --arg sha "$expected_sha" "length == 2 and ([.[].databaseId] | unique | length) == 2 and all(.[]; .headSha == \$sha and .conclusion == \"success\")" target/release-rehearsal-runs.jsonl'
```

Attach both exact URLs, both per-job import/`CACHED` excerpts, tag inventories,
and full version-ID before/delta/clean inventories to checkpoint evidence.
Never substitute `gh run list`.

- [ ] **Step 8: Delete the complete isolated version-ID delta**

Run:

```bash
rtk proxy sh -c 'set -eu; suffix=$(rtk sed -n "1p" target/release-rehearsal-suffix.txt); binaries="binaries$suffix"; ui="ui$suffix"; rtk sh scripts/inspect-registry-cache.sh target/release-cache-versions-after.json --all-version-ids; rtk jq -e -n --arg binaries "$binaries" --arg ui "$ui" --slurpfile before target/release-cache-versions-before.json --slurpfile first target/release-cache-versions-after-first.json --slurpfile after target/release-cache-versions-after.json '\''($before[0] | map(.id)) as $old | [$first[0][] | select((.id as $id | $old | index($id)) == null)] as $first_new | ($first[0] | map(.id)) as $first_ids | [$after[0][] | select((.id as $id | $first_ids | index($id)) == null)] as $second_new | [$after[0][] | select((.id as $id | $old | index($id)) == null)] as $all_new | if ($first_new | length) != 2 or ($second_new | length) != 2 or ($all_new | length) != 4 or any($first_new[], $second_new[]; (.tags | length) != 1 or (.tags[0] != $binaries and .tags[0] != $ui)) or (([$binaries,$ui] - ($first_new | map(.tags[]) | unique)) | length) != 0 or (([$binaries,$ui] - ($second_new | map(.tags[]) | unique)) | length) != 0 then error("cache write deltas are not exactly attributable") else $all_new end'\'' > target/release-cache-new-versions.json; rtk jq -r ".[].id" target/release-cache-new-versions.json > target/release-cache-new-version-ids.txt; while IFS= read -r id; do rtk gh api --method DELETE "/users/intezya/packages/container/komodo-build-cache/versions/$id" >/dev/null; done < target/release-cache-new-version-ids.txt; rtk sh scripts/inspect-registry-cache.sh target/release-cache-versions-clean.json --all-version-ids; rtk jq -e -n --slurpfile before target/release-cache-versions-before.json --slurpfile clean target/release-cache-versions-clean.json "(\$before[0] | map(.id) | sort) == (\$clean[0] | map(.id) | sort)"; rtk sh scripts/inspect-registry-cache.sh target/release-cache-after-delete.json "$binaries" "$ui"; rtk jq -e "length == 2 and all(.[]; .absent == true)" target/release-cache-after-delete.json'
```

Expected: the after-first snapshot attributes exactly two single-tag versions,
and the after-second snapshot attributes exactly two more. Only the two
after-first IDs may have become untagged; an arbitrary extra untagged version
fails the gate. All four attributed IDs are deleted, the remaining ID set exactly
matches the before snapshot, and both tags are absent. Never delete only the
currently tagged versions: the first writes may have become untagged.
Do **not** seed stable refs yet; Task 6 does that only after the cargo-chef
layout is reviewed and merged.

### Task 4: Make the release contract require a real cargo-chef dependency layer

**Files:**
- Modify: `scripts/check-release-targets.sh:85-105`
- Test: `scripts/check-release-targets.sh`

- [ ] **Step 1: Replace target-cache-mount assertions with final checkpoint-2 assertions**

Replace the current `cache_mount` loop with:

```sh
for chef_directive in \
  "cargo install cargo-chef --version 0.1.77 --locked" \
  "cargo chef prepare --recipe-path recipe.json" \
  "cargo chef cook --release --locked --recipe-path recipe.json" \
  "cargo build --release --locked" \
  "ARG SOURCE_REVISION=unknown" \
  'LABEL org.opencontainers.image.revision="$SOURCE_REVISION"'; do
  if ! grep -Fq "$chef_directive" bin/binaries.Dockerfile; then
    echo "bin/binaries.Dockerfile is missing $chef_directive" >&2
    exit 1
  fi
done

if grep -q "type=cache,target=/builder/target" bin/binaries.Dockerfile; then
  echo "cargo-chef target output must remain in the exported Docker layer" >&2
  exit 1
fi

for product_dockerfile in \
  ui/Dockerfile \
  bin/core/single-arch.Dockerfile \
  bin/periphery/single-arch.Dockerfile \
  bin/cli/single-arch.Dockerfile; do
  for provenance_directive in \
    "ARG SOURCE_REVISION=unknown" \
    'LABEL org.opencontainers.image.revision="$SOURCE_REVISION"'; do
    if ! grep -Fq "$provenance_directive" "$product_dockerfile"; then
      echo "$product_dockerfile is missing $provenance_directive" >&2
      exit 1
    fi
  done
done

require_count 3 'SOURCE_REVISION=${{ github.sha }}' "$release_workflow"
if ! grep -A1 -F '${{ matrix.build_args }}' "$release_workflow" \
  | grep -Fq 'SOURCE_REVISION=${{ github.sha }}'; then
  echo "release.yml does not append SOURCE_REVISION to matrix build args" >&2
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

for final_release_directive in \
  "          - seed" \
  "cache_mode == 'seed'" \
  "group: release-cache" \
  "queue: max" \
  "cancel-in-progress: false" \
  'EXPECTED_SHA: ${{ inputs.expected_sha }}' \
  'if [ "$GITHUB_SHA" != "$EXPECTED_SHA" ]; then'; do
  if ! grep -Fq "$final_release_directive" "$release_workflow"; then
    echo "$release_workflow is missing final policy $final_release_directive" >&2
    exit 1
  fi
done

require_count 2 "needs: preflight" "$release_workflow"
require_count 1 "push: \${{ github.event_name == 'push' }}" "$release_workflow"
require_count 2 "push: \${{ github.event_name == 'push' || inputs.cache_mode == 'seed' }}" "$release_workflow"
require_count 3 "type=registry,ref=ghcr.io/intezya/komodo-build-cache:" "$release_workflow"
require_count 3 "type=registry,mode=max,image-manifest=true,oci-mediatypes=true,ref=ghcr.io/intezya/komodo-build-cache:" "$release_workflow"
require_count 3 "cache-to: \${{ github.event_name == 'workflow_dispatch'" "$release_workflow"
require_count 1 "if: github.event_name == 'push' || inputs.cache_mode == 'seed'" "$release_workflow"
require_count 2 "matrix.cache_scope" "$release_workflow"
require_count 4 "komodo-build-cache:seed-binaries-{0}', inputs.dispatch_nonce" "$release_workflow"
require_count 2 "komodo-build-cache:seed-ui-{0}', inputs.dispatch_nonce" "$release_workflow"
require_count 2 "inputs.cache_mode == 'seed' && 'false' || 'mode=min'" "$release_workflow"

if grep -Fq 'komodo-build-cache:${{ matrix.name }}' "$release_workflow"; then
  echo "$release_workflow uses short product names instead of stable cache scopes" >&2
  exit 1
fi

if grep -Fq "type=gha,scope=" "$release_workflow" \
  || grep -Fq "type=gha,mode=max,scope=" "$release_workflow"; then
  echo "$release_workflow still contains checkpoint-1 GHA BuildKit scopes" >&2
  exit 1
fi

if ! grep -Fq -- "--all-version-ids" scripts/inspect-registry-cache.sh; then
  echo "cache inventory cannot expose superseded untagged versions" >&2
  exit 1
fi

for selector_guard in \
  'selected_sha=$(printf' \
  'if [ "$selected_sha" != "$expected_sha" ]; then' \
  'gh run cancel --repo "$repo" "$id"'; do
  if ! grep -Fq "$selector_guard" scripts/dispatch-workflow-run.sh; then
    echo "dispatch helper is missing $selector_guard" >&2
    exit 1
  fi
done

for cleanup_guard in \
  '.[0].tags | length' \
  'refusing to delete non-isolated bootstrap tag' \
  '/users/intezya/packages/container/komodo-build-cache/versions/$id'; do
  if ! grep -Fq "$cleanup_guard" scripts/delete-seed-bootstrap.sh; then
    echo "seed bootstrap cleanup is missing $cleanup_guard" >&2
    exit 1
  fi
done

for workflow_cleanup_guard in \
  "  packages: write" \
  "  seed_cleanup:" \
  "needs: [preflight, binaries, ui, images]" \
  "always() && github.event_name == 'workflow_dispatch' && inputs.cache_mode == 'seed'" \
  "sh scripts/delete-seed-bootstrap.sh" \
  "name: seed-bootstrap-cleanup"; do
  if ! grep -Fq "$workflow_cleanup_guard" "$release_workflow"; then
    echo "seed workflow cleanup is missing $workflow_cleanup_guard" >&2
    exit 1
  fi
done
```

Also require `--all-version-ids` in `scripts/inspect-registry-cache.sh`; tag
lookups alone cannot see superseded untagged versions. The three final registry assertions correspond to the binaries build, UI
build, and one matrix product-image build definition. Keep these exact-count
checks beside the existing repository/target checks. They are static
regression guards only: Actionlint proves YAML structure, while Tasks 6 and 12
prove actual ref selection, serialization, cache hits, and non-publication.

- [ ] **Step 2: Run the contract and verify it fails before the Dockerfile change**

Run:

```bash
rtk sh scripts/check-release-targets.sh
```

Expected: FAIL on the first absent cargo-chef or final-registry directive. Do
not weaken counts merely to make a partially migrated workflow pass.

### Task 5: Split dependency/application compilation and promote the reviewed registry policy

**Files:**
- Modify: `.github/workflows/release.yml`
- Verify: `bin/binaries.Dockerfile.dockerignore`
- Verify: `ui/Dockerfile.dockerignore`
- Verify: `bin/{core,periphery,cli}/single-arch.Dockerfile.dockerignore`
- Modify: `bin/binaries.Dockerfile:1-43`
- Modify: `ui/Dockerfile`
- Modify: `bin/core/single-arch.Dockerfile`
- Modify: `bin/periphery/single-arch.Dockerfile`
- Modify: `bin/cli/single-arch.Dockerfile`
- Create: `scripts/smoke-release-images.sh`
- Create: `scripts/probe-cargo-chef.sh`
- Create: `scripts/delete-seed-bootstrap.sh`
- Test: `scripts/check-release-targets.sh`

- [ ] **Step 1: Re-verify the effective BuildKit context allowlists**

Do not inspect or rely on a root `.dockerignore`. Confirm the five
Dockerfile-specific files named in the file map still begin with `*`, and that
`ui/Dockerfile.dockerignore` ends with:

```dockerignore
**/.env
**/.env.*
**/node_modules
**/dist
**/*.pem
**/*.key
```

Run the ignore-file portion of `scripts/check-release-targets.sh` before any
registry export. The binaries planner's `COPY . .` is acceptable because its
effective file is a checked-in allowlist of Cargo metadata and Rust workspace
sources; the UI exceptions are followed by recursive secret exclusions.

- [ ] **Step 2: Replace the builder setup with pinned chef and planner stages**

Use this complete stage prefix:

```dockerfile
# syntax=docker/dockerfile:1

FROM rust:1.95.0-bookworm AS chef

RUN cargo install cargo-chef --version 0.1.77 --locked

WORKDIR /builder

FROM chef AS planner

COPY . .

RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

COPY --from=planner /builder/recipe.json recipe.json

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
  --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
  cargo chef cook --release --locked --recipe-path recipe.json

COPY . .
```

The planner and builder use the same Rust 1.95 base and `/builder` working directory. Do not mount `/builder/target`; the `cargo chef cook` output must be part of the exported BuildKit layer.

- [ ] **Step 3: Keep one application build and the scratch output stage**

Use this build block after `COPY . .`:

```dockerfile
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
  --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
  cargo build --release --locked \
    -p komodo_core \
    -p komodo_periphery \
    -p komodo_cli && \
  strip \
    target/release/core \
    target/release/periphery \
    target/release/km && \
  mkdir -p /dist && \
  cp \
    target/release/core \
    target/release/periphery \
    target/release/km \
    /dist/

FROM scratch

ARG SOURCE_REVISION=unknown

COPY --from=builder /dist/core /core
COPY --from=builder /dist/periphery /periphery
COPY --from=builder /dist/km /km

LABEL org.opencontainers.image.source="https://github.com/intezya/komodo"
LABEL org.opencontainers.image.description="Komodo Binaries"
LABEL org.opencontainers.image.licenses="GPL-3.0"
LABEL org.opencontainers.image.revision="$SOURCE_REVISION"
```

- [ ] **Step 4: Carry the revision into every final product image**

Immediately after the final runtime `FROM` in each single-arch Dockerfile add:

```dockerfile
ARG SOURCE_REVISION=unknown
```

Append this beside the existing OCI labels in Core, Periphery, and CLI:

```dockerfile
LABEL org.opencontainers.image.revision="$SOURCE_REVISION"
```

In the final `FROM scratch` stage of `ui/Dockerfile`, add the same
`ARG SOURCE_REVISION=unknown` and OCI revision label. Task 2 already passes
`${{ github.sha }}` to binaries and UI. Change the product matrix build action
from its scalar `build-args` to:

```yaml
build-args: |
  ${{ matrix.build_args }}
  SOURCE_REVISION=${{ github.sha }}
```

The common line applies to Core, Periphery, and CLI, so provenance is
inspectable on binaries, UI, and all three runnable images, not only on an
intermediate builder stage.

- [ ] **Step 5: Promote tag reads and trusted-main seed writes to stable registry refs**

Extend `cache_mode.options` with `seed`. In preflight, preserve every SHA,
nonce, and trusted-main check, then validate exactly one of these branches:

```sh
case "$CACHE_MODE" in
  rehearsal)
    body=${CACHE_SUFFIX#-rehearsal-}
    if [ "$body" = "$CACHE_SUFFIX" ] || [ -z "$body" ]; then
      echo "rehearsal cache_suffix must start with -rehearsal-" >&2
      exit 1
    fi
    case "$body" in *[!a-z0-9-]*) exit 1 ;; esac
    ;;
  seed)
    if [ -n "$CACHE_SUFFIX" ]; then
      echo "stable seed requires an empty cache_suffix" >&2
      exit 1
    fi
    ;;
  *) echo "cache_mode must be rehearsal or seed" >&2; exit 1 ;;
esac
```

Keep the constant `group: release-cache`, `queue: max`, and
`cancel-in-progress: false` from checkpoint 1. It serializes tags, rehearsal writers with different suffixes,
and stable seeds, so no release-cache package mutation can interleave with the
full-version-ID snapshots. Then use this final policy:

```yaml
# binaries (use product/UI names and seed-ui in the UI job)
tags: |
  ${{ github.event_name == 'push' && format('ghcr.io/intezya/komodo-binaries:{0}', github.ref_name) || '' }}
  ${{ github.event_name == 'push' && 'ghcr.io/intezya/komodo-binaries:2' || '' }}
  ${{ github.event_name == 'workflow_dispatch' && inputs.cache_mode == 'seed' && format('ghcr.io/intezya/komodo-build-cache:seed-binaries-{0}', inputs.dispatch_nonce) || '' }}
push: ${{ github.event_name == 'push' || inputs.cache_mode == 'seed' }}
provenance: ${{ github.event_name == 'workflow_dispatch' && inputs.cache_mode == 'seed' && 'false' || 'mode=min' }}
cache-from: ${{ format('type=registry,ref=ghcr.io/intezya/komodo-build-cache:binaries{0}', github.event_name == 'workflow_dispatch' && inputs.cache_mode == 'rehearsal' && inputs.cache_suffix || '') }}
cache-to: ${{ github.event_name == 'workflow_dispatch' && format('type=registry,mode=max,image-manifest=true,oci-mediatypes=true,ref=ghcr.io/intezya/komodo-build-cache:binaries{0}', inputs.cache_mode == 'rehearsal' && inputs.cache_suffix || '') || '' }}

# product-image matrix
if: github.event_name == 'push' || inputs.cache_mode == 'seed'
cache-from: type=registry,ref=ghcr.io/intezya/komodo-build-cache:${{ matrix.cache_scope }}
cache-to: ${{ github.event_name == 'workflow_dispatch' && inputs.cache_mode == 'seed' && format('type=registry,mode=max,image-manifest=true,oci-mediatypes=true,ref=ghcr.io/intezya/komodo-build-cache:{0}', matrix.cache_scope) || '' }}
push: ${{ github.event_name == 'push' }}
```

Keep matrix `cache_scope` values `komodo-core`, `komodo-periphery`, and
`komodo-cli`; do not substitute the shorter matrix `name`. In every matrix
`build_args`, select the just-published product ref for a tag and the exact
temporary cache-package bootstrap ref for seed (and add the UI expression to
Core):

```yaml
BINARIES_IMAGE=${{ github.event_name == 'push' && format('ghcr.io/intezya/komodo-binaries:{0}', github.ref_name) || format('ghcr.io/intezya/komodo-build-cache:seed-binaries-{0}', inputs.dispatch_nonce) }}
UI_IMAGE=${{ github.event_name == 'push' && format('ghcr.io/intezya/komodo-ui:{0}', github.ref_name) || format('ghcr.io/intezya/komodo-build-cache:seed-ui-{0}', inputs.dispatch_nonce) }}
```

The two temporary images are necessary because the fork's product packages may
be absent before the first cache seed. Setting seed provenance to `false`
keeps each single-platform bootstrap ref a directly deletable image manifest.
A seed publishes no product image: it writes five stable cache manifests plus
two uniquely named bootstrap images inside `komodo-build-cache`, and Task 6
deletes those two images. Release tags read all five stable refs and pass an
empty `cache-to`; rehearsal remains `push:false`, writes only its two
suffix-scoped cache refs, and skips product images/releases.

- [ ] **Step 6: Add fail-closed bootstrap cleanup**

Create `scripts/delete-seed-bootstrap.sh`. It accepts absent tags so the same
helper is safe after a partially failed seed, but it deletes a present tag only
when exactly one package version owns it and that version has no other tag:

```sh
#!/usr/bin/env sh
set -eu

if [ "$#" -lt 3 ]; then
  echo "usage: $0 OUTPUT_JSON TAG TAG..." >&2
  exit 2
fi

output=$1
shift
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
script_dir=$(CDPATH= cd -P "$(dirname "$0")" && pwd)

sh "$script_dir/inspect-registry-cache.sh" \
  "$tmp/before.json" --all-version-ids
: > "$tmp/deleted.jsonl"

for tag do
  row=$(jq -c --arg tag "$tag" \
    '[.[] | select(any(.tags[]?; . == $tag))]' "$tmp/before.json")
  count=$(printf '%s' "$row" | jq 'length')
  if [ "$count" -eq 0 ]; then
    jq -cn --arg tag "$tag" \
      '{tag:$tag,absent_before:true,deleted_id:null}' \
      >> "$tmp/deleted.jsonl"
    continue
  fi
  if [ "$count" -ne 1 ] \
    || [ "$(printf '%s' "$row" | jq '.[0].tags | length')" -ne 1 ] \
    || [ "$(printf '%s' "$row" | jq -r '.[0].tags[0]')" != "$tag" ]; then
    echo "refusing to delete non-isolated bootstrap tag $tag" >&2
    exit 1
  fi
  id=$(printf '%s' "$row" | jq -r '.[0].id')
  gh api --method DELETE \
    "/users/intezya/packages/container/komodo-build-cache/versions/$id" \
    >/dev/null
  jq -cn --arg tag "$tag" --argjson id "$id" \
    '{tag:$tag,absent_before:false,deleted_id:$id}' \
    >> "$tmp/deleted.jsonl"
done

sh "$script_dir/inspect-registry-cache.sh" \
  "$tmp/after.json" --all-version-ids
for tag do
  if jq -e --arg tag "$tag" \
    'any(.[]; any(.tags[]?; . == $tag))' "$tmp/after.json" >/dev/null; then
    echo "bootstrap tag $tag remains after cleanup" >&2
    exit 1
  fi
done
jq -s 'sort_by(.tag)' "$tmp/deleted.jsonl" > "$tmp/output.json"
mv "$tmp/output.json" "$output"
```

This helper intentionally does not delete untagged or stable versions. The
paired rehearsal has stronger after-first/after-second attribution for its
superseded untagged versions; seed bootstrap writes each unique tag only once.

Add a final workflow-owned cleanup job so local process interruption cannot
leave a dispatched seed running after an early “absent” check:

```yaml
  seed_cleanup:
    name: Delete seed bootstrap images
    if: ${{ always() && github.event_name == 'workflow_dispatch' && inputs.cache_mode == 'seed' }}
    needs: [preflight, binaries, ui, images]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - name: Delete exact temporary seed images
        env:
          GH_TOKEN: ${{ github.token }}
          NONCE: ${{ inputs.dispatch_nonce }}
        run: |
          binaries="seed-binaries-$NONCE"
          ui="seed-ui-$NONCE"
          sh scripts/inspect-registry-cache.sh \
            seed-bootstrap-versions-before.json --all-version-ids
          sh scripts/delete-seed-bootstrap.sh \
            seed-bootstrap-cleanup.json "$binaries" "$ui"
          sh scripts/inspect-registry-cache.sh \
            seed-bootstrap-after.json "$binaries" "$ui"
          jq -e 'length == 2 and all(.[]; .absent == true)' \
            seed-bootstrap-after.json
      - name: Upload bootstrap cleanup evidence
        if: ${{ always() }}
        uses: actions/upload-artifact@v7
        with:
          name: seed-bootstrap-cleanup
          if-no-files-found: error
          path: |
            seed-bootstrap-versions-before.json
            seed-bootstrap-cleanup.json
            seed-bootstrap-after.json
```

Keep workflow `permissions.packages: write`. Before enabling seed, record in
the checkpoint evidence that `komodo-build-cache` grants this repository
Actions admin access; a package first published by this workflow receives that
access by default. Missing admin access blocks seed. The cleanup job waits for all
producer and product-matrix jobs via `needs`, runs even when one fails, and its
failure makes the workflow fail. Tag and rehearsal runs skip it.

- [ ] **Step 7: Add one reusable cold/warm final-image smoke**

Create `scripts/smoke-release-images.sh` using only portable Docker/shell
commands (no `rtk` inside the committed helper):

```sh
#!/usr/bin/env sh
set -eu

if [ "$#" -ne 4 ]; then
  echo "usage: $0 BINARIES_IMAGE REVISION IMAGE_PREFIX EXPECTED_VERSION" >&2
  exit 2
fi

binaries=$1
revision=$2
prefix=$3
expected_version=$4
builder=${BUILDX_BUILDER:-}
no_cache=${NO_CACHE:-false}
size_output=${IMAGE_SIZE_OUT:-}
version_output=${VERSION_EVIDENCE_OUT:-}
ui="$prefix-ui"
core_image="$prefix-core"
periphery_image="$prefix-periphery"
cli_image="$prefix-cli"

case "$no_cache" in true|false) ;; *) echo "NO_CACHE must be true or false" >&2; exit 2 ;; esac

build_with_builder() {
  if [ -n "$builder" ] && [ "$no_cache" = true ]; then
    docker buildx build --builder "$builder" --no-cache --load "$@"
  elif [ -n "$builder" ]; then
    docker buildx build --builder "$builder" --load "$@"
  elif [ "$no_cache" = true ]; then
    docker buildx build --no-cache --load "$@"
  else
    docker buildx build --load "$@"
  fi
}

build_with_host() {
  if [ "$no_cache" = true ]; then
    docker build --no-cache "$@"
  else
    docker build "$@"
  fi
}

build_with_builder --platform linux/amd64 --file ui/Dockerfile \
  --build-arg SOURCE_REVISION="$revision" --tag "$ui" .
build_with_host --platform linux/amd64 \
  --file bin/core/single-arch.Dockerfile \
  --build-arg BINARIES_IMAGE="$binaries" \
  --build-arg UI_IMAGE="$ui" \
  --build-arg SOURCE_REVISION="$revision" \
  --tag "$core_image" .
build_with_host --platform linux/amd64 \
  --file bin/periphery/single-arch.Dockerfile \
  --build-arg BINARIES_IMAGE="$binaries" \
  --build-arg SOURCE_REVISION="$revision" \
  --tag "$periphery_image" .
build_with_host --platform linux/amd64 \
  --file bin/cli/single-arch.Dockerfile \
  --build-arg BINARIES_IMAGE="$binaries" \
  --build-arg SOURCE_REVISION="$revision" \
  --tag "$cli_image" .

cli_version=$(docker run --rm --platform linux/amd64 \
  --entrypoint /usr/local/bin/km "$cli_image" --version)
periphery_version=$(docker run --rm --platform linux/amd64 \
  --entrypoint /usr/local/bin/periphery \
  "$periphery_image" --version)
normalize_version() {
  printf '%s\n' "$1" | awk \
    'NF == 2 {value=$2; sub(/^v/, "", value); print value}'
}
test "$(normalize_version "$cli_version")" = "$expected_version"
test "$(normalize_version "$periphery_version")" = "$expected_version"

suffix=$$
network="komodo-smoke-$suffix"
mongo="komodo-mongo-$suffix"
core="komodo-core-$suffix"
cleanup() {
  docker rm -f "$core" "$mongo" >/dev/null 2>&1 || true
  docker network rm "$network" >/dev/null 2>&1 || true
}
trap cleanup EXIT HUP INT TERM

docker network create "$network" >/dev/null
docker run -d --platform linux/amd64 --name "$mongo" \
  --network "$network" mongo:8 >/dev/null
attempt=0
until [ "$(docker exec "$mongo" mongosh --quiet --eval \
  'print(db.runCommand({ping:1}).ok)' 2>/dev/null || true)" = 1 ]; do
  attempt=$((attempt + 1))
  [ "$attempt" -lt 60 ]
  sleep 1
done

docker run -d --platform linux/amd64 --name "$core" \
  --network "$network" \
  -e KOMODO_DATABASE_URI="mongodb://$mongo:27017" \
  "$core_image" >/dev/null
attempt=0
while :; do
  code=$(docker run --rm --network "$network" \
    curlimages/curl:8.12.1 --silent --show-error \
    --output /dev/null --write-out '%{http_code}' \
    "http://$core:9120/version" 2>/dev/null || true)
[ "$code" = 200 ] && break
  attempt=$((attempt + 1))
  [ "$attempt" -lt 60 ]
  sleep 1
done
core_version=$(docker run --rm --network "$network" \
  curlimages/curl:8.12.1 --fail --silent --show-error \
  "http://$core:9120/version")
test "$core_version" = "$expected_version"
sleep 5
[ "$code" = 200 ]
[ "$(docker inspect "$core" --format '{{.State.Running}}')" = true ]
docker logs "$core" 2>&1 | grep -F 'Komodo Core version'

if [ -n "$version_output" ]; then
  version_tmp="$version_output.tmp.$$"
  {
    printf 'cli\t%s\t%s\n' "$expected_version" "$cli_version"
    printf 'periphery\t%s\t%s\n' \
      "$expected_version" "$periphery_version"
    printf 'core\t%s\t%s\n' "$expected_version" "$core_version"
  } > "$version_tmp"
  mv "$version_tmp" "$version_output"
fi

for image in "$binaries" "$ui" "$core_image" \
  "$periphery_image" "$cli_image"; do
  [ "$(docker image inspect "$image" --format \
    '{{ index .Config.Labels "org.opencontainers.image.revision" }}')" \
    = "$revision" ]
done

if [ -n "$size_output" ]; then
  size_tmp="$size_output.tmp.$$"
  trap 'rm -f "$size_tmp"; cleanup' EXIT HUP INT TERM
  : > "$size_tmp"
  for pair in \
    "binaries=$binaries" "ui=$ui" "core=$core_image" \
    "periphery=$periphery_image" "cli=$cli_image"; do
    role=${pair%%=*}
    image=${pair#*=}
    bytes=$(docker image inspect "$image" --format '{{.Size}}')
    printf '%s\t%s\t%s\t%s\n' "$role" "$image" "$revision" "$bytes" \
      >> "$size_tmp"
  done
  mv "$size_tmp" "$size_output"
fi
```

The Mongo ping proves the dependency is healthy; the Core HTTP probe cannot
succeed with exact HTTP 200 until the listener is available, and its body must
equal the same workspace version parsed exactly from CLI and Periphery
`--version` output. The probe uses the
public `GET /version` route rather than accepting any nonzero response from a
POST. `BUILDX_BUILDER` keeps the UI build on the caller's isolated builder,
`NO_CACHE=true` applies to both the isolated and host build paths, and
`IMAGE_SIZE_OUT` atomically records all five loaded-image sizes. Core,
Periphery, and CLI intentionally use the host Docker builder: a
`docker-container` BuildKit instance cannot resolve the binaries/UI tags that
`--load` placed in the daemon image store. `NO_CACHE=true` is passed to both
builder paths, so the cold proof still disables reuse for all five images. CLI
and Periphery bypass their image CMD/entrypoint and execute the intended
binary.

- [ ] **Step 8: Add one isolated-builder cargo-chef probe**

Create `scripts/probe-cargo-chef.sh` with no `rtk` inside. It owns one builder
for its entire lifetime and uses copied contexts so the working tree remains
untouched:

```sh
#!/usr/bin/env sh
set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: $0 OUTPUT_DIRECTORY" >&2
  exit 2
fi

output=$1
mkdir -p "$output"
root=$(pwd)
tmp=$(mktemp -d)
builder="komodo-chef-probe-$$"
cleanup() {
  docker buildx rm "$builder" >/dev/null 2>&1 || true
  rm -rf "$tmp"
}
trap cleanup EXIT HUP INT TERM

docker buildx create --name "$builder" --driver docker-container --use >/dev/null
docker buildx inspect "$builder" --bootstrap >/dev/null
printf '%s\n' "$builder" > "$output/builder-name.txt"

for probe in cold source dependency lock; do
  mkdir -p "$tmp/$probe"
  rsync -a --exclude .git --exclude target --exclude .worktrees \
    "$root/" "$tmp/$probe/"
done
printf '\n// cargo-chef source-only probe\n' \
  >> "$tmp/source/bin/cli/src/main.rs"
printf '\nsha1.workspace = true\n' \
  >> "$tmp/dependency/bin/cli/Cargo.toml"
(cd "$tmp/dependency" && cargo metadata --offline --format-version 1 >/dev/null)
(cd "$tmp/dependency" && cargo metadata --offline --locked --format-version 1 >/dev/null)
if cmp "$root/Cargo.lock" "$tmp/dependency/Cargo.lock" >/dev/null; then
  echo "dependency probe did not update Cargo.lock" >&2
  exit 1
fi
printf '\n# cargo-chef lockfile-only probe\n' \
  >> "$tmp/lock/Cargo.lock"

build_probe() {
  probe=$1
  shift
  docker buildx build --builder "$builder" "$@" \
    --file "$tmp/$probe/bin/binaries.Dockerfile" \
    --platform linux/amd64 \
    --build-arg SOURCE_REVISION="$probe-probe" \
    --progress=plain --load \
    --tag "komodo-binaries:chef-$probe" \
    "$tmp/$probe" > "$output/$probe.log" 2>&1
}

build_probe cold --no-cache
build_probe source
build_probe dependency
build_probe lock

vertex_status() {
  log=$1
  command=$2
  vertex=$(awk -v command="$command" 'index($0, command) {print $1; exit}' "$log")
  [ -n "$vertex" ]
  awk -v vertex="$vertex" \
    '$1 == vertex && ($2 == "DONE" || $2 == "CACHED") {status=$2; seconds=$3} END {if (!status) exit 1; sub(/s$/, "", seconds); print status "\t" (seconds == "" ? 0 : seconds)}' \
    "$log"
}

printf 'probe\tchef_status\tchef_seconds\tapp_status\tapp_seconds\tbinaries_bytes\n' \
  > "$output/cargo-timings.tsv"
for probe in cold source dependency lock; do
  chef=$(vertex_status "$output/$probe.log" 'cargo chef cook --release --locked')
  app=$(vertex_status "$output/$probe.log" 'cargo build --release --locked')
  size=$(docker image inspect "komodo-binaries:chef-$probe" --format '{{.Size}}')
  printf '%s\t%s\t%s\t%s\n' "$probe" "$chef" "$app" "$size" \
    >> "$output/cargo-timings.tsv"
done

awk -F '\t' '$1 == "cold" {found=1; ok=($2 == "DONE" && $4 == "DONE")} END {exit !(found && ok)}' \
  "$output/cargo-timings.tsv"
awk -F '\t' '$1 == "source" {found=1; ok=($2 == "CACHED" && $4 == "DONE")} END {exit !(found && ok)}' \
  "$output/cargo-timings.tsv"
for probe in dependency lock; do
  awk -F '\t' -v probe="$probe" \
    '$1 == probe {found=1; ok=($2 == "DONE" && $4 == "DONE")} END {exit !(found && ok)}' \
    "$output/cargo-timings.tsv"
done

awk -F '\t' 'NR > 1 && NF == 6 && $6 ~ /^[0-9]+$/ && $6 > 0 {count++} END {exit count != 4}' \
  "$output/cargo-timings.tsv"

workspace_version=$(cargo metadata --no-deps --format-version 1 \
  | jq -r '.packages[] | select(.name == "komodo_core") | .version')
test -n "$workspace_version"

BUILDX_BUILDER="$builder" NO_CACHE=true \
  IMAGE_SIZE_OUT="$output/cold-image-sizes.tsv" \
  VERSION_EVIDENCE_OUT="$output/cold-versions.tsv" \
  sh scripts/smoke-release-images.sh \
  komodo-binaries:chef-cold cold-probe komodo-chef-cold \
  "$workspace_version"
BUILDX_BUILDER="$builder" NO_CACHE=false \
  IMAGE_SIZE_OUT="$output/source-image-sizes.tsv" \
  VERSION_EVIDENCE_OUT="$output/source-versions.tsv" \
  sh scripts/smoke-release-images.sh \
  komodo-binaries:chef-source source-probe komodo-chef-source \
  "$workspace_version"
cmp "$output/cold-versions.tsv" "$output/source-versions.tsv"

for sizes in cold-image-sizes.tsv source-image-sizes.tsv; do
  awk -F '\t' 'NF == 4 && $4 ~ /^[0-9]+$/ && $4 > 0 {count++} END {exit count != 5}' \
    "$output/$sizes"
done
```

The dependency probe changes a real member manifest, refreshes only its copied
lockfile with offline metadata, and re-validates that copy with `--locked`;
the lock probe changes only valid lockfile comment content and proves the
recipe is sensitive to the lockfile itself. Neither probe is accepted unless its `cargo chef cook` vertex
is `DONE`. The cold run disables cache for the binaries build and for every
image built by the smoke helper. The same isolated builder then supplies the
source, dependency, lock, and both UI smoke builds; the daemon builder supplies
the three final images so their locally loaded `FROM` tags resolve. The
isolated builder is removed only by the final trap.

- [ ] **Step 9: Run the static contracts and helper syntax checks**

Run:

```bash
rtk sh scripts/check-release-targets.sh
rtk sh -n scripts/smoke-release-images.sh
rtk sh -n scripts/probe-cargo-chef.sh
rtk sh -n scripts/delete-seed-bootstrap.sh
rtk actionlint -ignore 'unexpected key "queue"' .github/workflows/release.yml
```

Expected: all static checks pass. The narrow actionlint ignore covers only its
schema lag for GitHub's supported `concurrency.queue`; the checker validates
the exact `queue: max` policy text. It does not claim that a cache exists or was
reused.

- [ ] **Step 10: Run the complete four-probe and all-five-image experiment**

Run:

```bash
rtk chmod +x scripts/smoke-release-images.sh scripts/probe-cargo-chef.sh
rtk sh scripts/probe-cargo-chef.sh target/cargo-chef-probe
rtk column -t -s $'\t' target/cargo-chef-probe/cargo-timings.tsv
rtk column -t -s $'\t' target/cargo-chef-probe/cold-image-sizes.tsv
rtk column -t -s $'\t' target/cargo-chef-probe/source-image-sizes.tsv
```

Expected: `builder-name.txt` records the one builder passed by the same helper
to all four cargo builds and both UI smoke builds; cold/dependency/lock chef
vertices are `DONE`, source chef is `CACHED`, and every application vertex is
`DONE`. Both size files contain exactly binaries, UI, Core, Periphery, and
CLI. The cold helper invocation passes `--no-cache` to the isolated UI build
and all three host product builds, while the preceding binaries probe is also
`--no-cache`. Core `GET /version` returns exactly HTTP 200, and all five images carry
the requested revision.

- [ ] **Step 11: Commit checkpoint 2 implementation**

```bash
rtk git add .github/workflows/release.yml bin/binaries.Dockerfile ui/Dockerfile bin/core/single-arch.Dockerfile bin/periphery/single-arch.Dockerfile bin/cli/single-arch.Dockerfile scripts/check-release-targets.sh scripts/smoke-release-images.sh scripts/probe-cargo-chef.sh scripts/delete-seed-bootstrap.sh
rtk git commit -m "build: cache Rust dependencies with cargo-chef"
```

### Task 6: Prove cargo-chef reuse, then seed all five stable refs once

**Files:**
- Verify: `.github/workflows/release.yml`
- Verify: `bin/binaries.Dockerfile`
- Verify: `scripts/dispatch-workflow-run.sh`
- Verify: `scripts/snapshot-release-state.sh`
- Verify: `scripts/inspect-registry-cache.sh`
- Verify: `scripts/delete-seed-bootstrap.sh`

Run this task after the Task 5 checkpoint merges to `main`.

- [ ] **Step 1: Re-check the hard package deletion prerequisite**

```bash
rtk gh api -i user 2>&1 | rtk rg -qi '^x-oauth-scopes:.*(read|write):packages'
rtk gh api -i user 2>&1 | rtk rg -qi '^x-oauth-scopes:.*delete:packages'
rtk gh auth token | rtk docker login ghcr.io --username intezya --password-stdin
```

Expected: all commands succeed. Missing `delete:packages` blocks this task;
do not create disposable refs and promise a best-effort cleanup.

- [ ] **Step 2: Generate one absent-checked batch and immutable SHA**

```bash
rtk proxy sh -c 'set -eu; rtk mkdir -p target; batch="p1-chef-$(rtk date +%s)-$$"; suffix="-rehearsal-$batch"; expected_sha=$(rtk gh api repos/intezya/komodo/commits/main --jq .sha); rtk printf "%s\n" "$suffix" > target/release-chef-suffix.txt; rtk printf "%s\n" "$expected_sha" > target/release-chef-sha.txt; rtk sh scripts/snapshot-release-state.sh target/release-chef-products-before.json; rtk sh scripts/inspect-registry-cache.sh target/release-chef-disposable-before.json "binaries$suffix" "ui$suffix"; rtk jq -e "length == 2 and all(.[]; .absent == true)" target/release-chef-disposable-before.json; rtk sh scripts/inspect-registry-cache.sh target/release-chef-cache-versions-before.json --all-version-ids'
```

Expected: one unique suffix and one 40-character SHA are generated once and
saved. If either disposable tag exists, generate a new batch; never delete an
unknown colliding ref.

- [ ] **Step 3: Run the paired rehearsal with that exact suffix and SHA**

```bash
rtk proxy sh -c 'set -eu; suffix=$(rtk sed -n "1p" target/release-chef-suffix.txt); expected_sha=$(rtk sed -n "1p" target/release-chef-sha.txt); rtk truncate -s 0 target/release-chef-run-ids.txt; for ordinal in first second; do nonce="p1-chef-$ordinal-$(rtk date +%s)-$$"; title="Release dispatch=$nonce cache=rehearsal suffix=$suffix sha=$expected_sha"; id=$(rtk sh scripts/dispatch-workflow-run.sh intezya/komodo release.yml main "$title" "$expected_sha" -f cache_mode=rehearsal -f cache_suffix="$suffix" -f dispatch_nonce="$nonce" -f expected_sha="$expected_sha"); rtk printf "%s\n" "$id" >> target/release-chef-run-ids.txt; rtk gh run watch --repo intezya/komodo "$id" --exit-status; rtk proxy gh run view --repo intezya/komodo "$id" --log > "target/release-chef-$ordinal.log"; if [ "$ordinal" = first ]; then rtk sh scripts/inspect-registry-cache.sh target/release-chef-cache-versions-after-first.json --all-version-ids; fi; done'
rtk proxy sh -c 'set -eu; suffix=$(rtk sed -n "1p" target/release-chef-suffix.txt); id=$(rtk sed -n "2p" target/release-chef-run-ids.txt); for scope in binaries ui; do job_name="Build and publish $scope"; job_id=$(rtk gh run view --repo intezya/komodo "$id" --json jobs --jq "[.jobs[] | select(.name == \"$job_name\")] | if length == 1 then .[0].databaseId else error(\"expected one job\") end"); rtk proxy gh run view --repo intezya/komodo --job "$job_id" --log > "target/release-chef-second-$scope.log"; rtk rg -F "importing cache manifest from ghcr.io/intezya/komodo-build-cache:$scope$suffix" "target/release-chef-second-$scope.log"; rtk rg -n "CACHED" "target/release-chef-second-$scope.log"; done; rtk rg -U "cargo chef cook(.|\n){0,800}CACHED" target/release-chef-second-binaries.log'
rtk proxy sh -c 'set -eu; while IFS= read -r id; do rtk gh run view --repo intezya/komodo "$id" --json databaseId,headSha,conclusion,startedAt,updatedAt,url; done < target/release-chef-run-ids.txt' | rtk proxy tee target/release-chef-rehearsal-runs.jsonl
```

Expected: two unique successful IDs use the same saved suffix and SHA. Each
second-run job imports its exact disposable ref; binaries specifically reports
the `cargo chef cook` vertex as `CACHED`.

- [ ] **Step 4: Inventory and delete the complete isolated version-ID delta**

```bash
rtk proxy sh -c 'set -eu; suffix=$(rtk sed -n "1p" target/release-chef-suffix.txt); binaries="binaries$suffix"; ui="ui$suffix"; rtk sh scripts/inspect-registry-cache.sh target/release-chef-disposable-after.json "$binaries" "$ui"; rtk jq -e "length == 2 and all(.[]; .absent == false and .layer_bytes > 0)" target/release-chef-disposable-after.json; rtk sh scripts/inspect-registry-cache.sh target/release-chef-cache-versions-after.json --all-version-ids; rtk jq -e -n --arg binaries "$binaries" --arg ui "$ui" --slurpfile before target/release-chef-cache-versions-before.json --slurpfile first target/release-chef-cache-versions-after-first.json --slurpfile after target/release-chef-cache-versions-after.json '\''($before[0] | map(.id)) as $old | [$first[0][] | select((.id as $id | $old | index($id)) == null)] as $first_new | ($first[0] | map(.id)) as $first_ids | [$after[0][] | select((.id as $id | $first_ids | index($id)) == null)] as $second_new | [$after[0][] | select((.id as $id | $old | index($id)) == null)] as $all_new | if ($first_new | length) != 2 or ($second_new | length) != 2 or ($all_new | length) != 4 or any($first_new[], $second_new[]; (.tags | length) != 1 or (.tags[0] != $binaries and .tags[0] != $ui)) or (([$binaries,$ui] - ($first_new | map(.tags[]) | unique)) | length) != 0 or (([$binaries,$ui] - ($second_new | map(.tags[]) | unique)) | length) != 0 then error("cache write deltas are not exactly attributable") else $all_new end'\'' > target/release-chef-new-versions.json; rtk jq -r ".[].id" target/release-chef-new-versions.json > target/release-chef-new-version-ids.txt; while IFS= read -r id; do rtk gh api --method DELETE "/users/intezya/packages/container/komodo-build-cache/versions/$id" >/dev/null; done < target/release-chef-new-version-ids.txt; rtk sh scripts/inspect-registry-cache.sh target/release-chef-cache-versions-clean.json --all-version-ids; rtk jq -e -n --slurpfile before target/release-chef-cache-versions-before.json --slurpfile clean target/release-chef-cache-versions-clean.json "(\$before[0] | map(.id) | sort) == (\$clean[0] | map(.id) | sort)"; rtk sh scripts/inspect-registry-cache.sh target/release-chef-disposable-deleted.json "$binaries" "$ui"; rtk jq -e "length == 2 and all(.[]; .absent == true)" target/release-chef-disposable-deleted.json'
```

Expected: every new version created during the exact pair is scoped to the two
unique tags or is an untagged superseded first write. All delta IDs are
deleted, the complete ID set returns to its before snapshot, and both tags are
absent.

- [ ] **Step 5: Snapshot five stable refs and absent-check two bootstrap refs**

```bash
rtk proxy sh -c 'set -eu; nonce="p1-chef-seed-$(rtk date +%s)-$$"; rtk printf "%s\n" "$nonce" > target/release-chef-seed-nonce.txt; binaries="seed-binaries-$nonce"; ui="seed-ui-$nonce"; rtk sh scripts/inspect-registry-cache.sh target/release-seed-bootstrap-before.json "$binaries" "$ui"; rtk jq -e "length == 2 and all(.[]; .absent == true)" target/release-seed-bootstrap-before.json; rtk sh scripts/inspect-registry-cache.sh target/release-stable-before-seed.json binaries ui komodo-core komodo-periphery komodo-cli; rtk jq -e "length == 5 and ([.[].tag] | unique | length) == 5" target/release-stable-before-seed.json; rtk sh scripts/inspect-registry-cache.sh target/release-seed-cache-versions-before.json --all-version-ids'
```

This snapshot is mandatory whether stable refs are present or absent. The seed
nonce is generated once, both `seed-*-<nonce>` tags must be absent, and the
same nonce is passed to workflow inputs and matrix base-image expressions.

- [ ] **Step 6: Dispatch, prove, and always clean one serialized seed**

Dispatch the seed and wait for its workflow-owned `seed_cleanup` job. Do not
cancel the GitHub run from a local signal handler: if the local shell receives
`EXIT`, `HUP`, `INT`, or `TERM` before it learns the run ID, the already
dispatched workflow still waits for all producers and deletes its own temporary
refs. On normal completion, download the cleanup artifact before accepting the
run:

```bash
rtk proxy sh -c 'set -eu; expected_sha=$(rtk sed -n "1p" target/release-chef-sha.txt); nonce=$(rtk sed -n "1p" target/release-chef-seed-nonce.txt); title="Release dispatch=$nonce cache=seed suffix= sha=$expected_sha"; id=$(rtk sh scripts/dispatch-workflow-run.sh intezya/komodo release.yml main "$title" "$expected_sha" -f cache_mode=seed -f cache_suffix= -f dispatch_nonce="$nonce" -f expected_sha="$expected_sha"); rtk printf "%s\n" "$id" > target/release-chef-seed-id.txt; status=0; rtk gh run watch --repo intezya/komodo "$id" --exit-status || status=$?; rtk proxy gh run view --repo intezya/komodo "$id" --log > target/release-chef-seed.log || true; rtk rm -rf target/release-seed-cleanup-artifact; rtk gh run download --repo intezya/komodo "$id" --name seed-bootstrap-cleanup --dir target/release-seed-cleanup-artifact; rtk cp target/release-seed-cleanup-artifact/seed-bootstrap-cleanup.json target/release-seed-bootstrap-cleanup.json; rtk cp target/release-seed-cleanup-artifact/seed-bootstrap-after.json target/release-seed-bootstrap-after.json; rtk cp target/release-seed-cleanup-artifact/seed-bootstrap-versions-before.json target/release-seed-cache-versions.json; test "$status" = 0'
rtk proxy sh -c 'set -eu; nonce=$(rtk sed -n "1p" target/release-chef-seed-nonce.txt); binaries="seed-binaries-$nonce"; ui="seed-ui-$nonce"; rtk jq -e -n --arg binaries "$binaries" --arg ui "$ui" --slurpfile versions target/release-seed-cache-versions.json '\''[$versions[0][] | select(any(.tags[]?; . == $binaries or . == $ui))] | if length != 2 or any(.[]; (.tags | length) != 1) or (([$binaries,$ui] - (map(.tags[]) | unique)) | length) != 0 then error("bootstrap versions were not two isolated single-tag images before workflow cleanup") else . end'\'' > target/release-seed-bootstrap-present.json; rtk jq -e "length == 2 and all(.[]; .absent_before == false and .deleted_id != null)" target/release-seed-bootstrap-cleanup.json; rtk jq -e "length == 2 and all(.[]; .absent == true)" target/release-seed-bootstrap-after.json; rtk sh scripts/inspect-registry-cache.sh target/release-seed-cache-versions-clean.json --all-version-ids'
rtk proxy sh -c 'set -eu; nonce=$(rtk sed -n "1p" target/release-chef-seed-nonce.txt); binaries="seed-binaries-$nonce"; ui="seed-ui-$nonce"; id=$(rtk sed -n "1p" target/release-chef-seed-id.txt); rtk sh scripts/inspect-registry-cache.sh target/release-stable-after-seed.json binaries ui komodo-core komodo-periphery komodo-cli; rtk jq -e -n --slurpfile before target/release-stable-before-seed.json --slurpfile after target/release-stable-after-seed.json '\''$after[0] as $a | $before[0] as $b | ($a | length) == 5 and all($a[]; .absent == false and .layer_bytes > 0) and all($a[]; . as $new | ($b | map(select(.tag == $new.tag))[0]) as $old | ($old.absent == true or $new.id != $old.id or $new.updated_at != $old.updated_at or $new.layer_bytes != $old.layer_bytes))'\''; for image in core periphery cli; do job_name="Publish $image image"; job_id=$(rtk gh run view --repo intezya/komodo "$id" --json jobs --jq "[.jobs[] | select(.name == \"$job_name\")] | if length == 1 then .[0].databaseId else error(\"expected one job\") end"); rtk proxy gh run view --repo intezya/komodo --job "$job_id" --log > "target/release-chef-seed-$image.log"; rtk rg -F "ghcr.io/intezya/komodo-build-cache:$binaries" "target/release-chef-seed-$image.log"; if [ "$image" = core ]; then rtk rg -F "ghcr.io/intezya/komodo-build-cache:$ui" "target/release-chef-seed-$image.log"; fi; done'
```

Expected: all five stable refs are present with nonzero compressed OCI layer
bytes and each changed from its own before record. Every product matrix job log
contains its exact temporary binaries ref, Core also contains the exact UI ref,
and both single-tag bootstrap versions are deleted by the final workflow job.
If dispatch, a producer, stable inventory, or matrix proof fails, the same
`always()` cleanup job runs and the workflow cannot finish successfully without
an uploaded absence proof. Do not retry until that artifact or a manual helper
run proves both refs absent.
Manifest byte length remains diagnostic only, never the cache-size metric.

- [ ] **Step 7: Re-check seed artifacts after cleanup**

```bash
rtk jq -e 'length == 5 and all(.[]; .absent == false and .layer_bytes > 0)' target/release-stable-after-seed.json
rtk jq -e 'length == 2 and all(.[]; .absent_before == false and .deleted_id != null)' target/release-seed-bootstrap-cleanup.json
rtk jq -e 'length == 2 and all(.[]; .absent == true)' target/release-seed-bootstrap-after.json
rtk proxy sh -c 'set -eu; nonce=$(rtk sed -n "1p" target/release-chef-seed-nonce.txt); binaries="seed-binaries-$nonce"; ui="seed-ui-$nonce"; rtk jq -e --arg binaries "$binaries" --arg ui "$ui" "length == 2 and all(.[]; (.tags | length) == 1) and (([\$binaries,\$ui] - (map(.tags[]) | unique)) | length) == 0" target/release-seed-bootstrap-present.json; for image in core periphery cli; do rtk rg -F "ghcr.io/intezya/komodo-build-cache:$binaries" "target/release-chef-seed-$image.log"; done; rtk rg -F "ghcr.io/intezya/komodo-build-cache:$ui" target/release-chef-seed-core.log'
rtk jq -e -n --slurpfile before target/release-seed-cache-versions-before.json --slurpfile clean target/release-seed-cache-versions-clean.json '($before[0] | map(.id)) as $old | [$clean[0][] | select((.id as $id | $old | index($id)) == null)] | all(.[]; (.tags | length) > 0 and all(.tags[]; . == "binaries" or . == "ui" or . == "komodo-core" or . == "komodo-periphery" or . == "komodo-cli"))'
rtk rg -F 'exporting cache to registry' target/release-chef-seed.log
```

Expected: stable refs remain, both temporary refs remain absent, and cleanup
evidence records the two exact deleted version IDs.

- [ ] **Step 8: Prove no product package or release changed and save exact runs**

```bash
rtk sh scripts/snapshot-release-state.sh target/release-chef-products-after.json
rtk cmp target/release-chef-products-before.json target/release-chef-products-after.json
rtk proxy sh -c 'set -eu; for file in target/release-chef-run-ids.txt target/release-chef-seed-id.txt; do while IFS= read -r id; do rtk gh run view --repo intezya/komodo "$id" --json databaseId,headSha,conclusion,url; done < "$file"; done' | rtk proxy tee target/release-chef-runs.jsonl
rtk proxy sh -c 'set -eu; expected_sha=$(rtk sed -n "1p" target/release-chef-sha.txt); rtk jq -se --arg sha "$expected_sha" "length == 3 and ([.[].databaseId] | unique | length) == 3 and all(.[]; .headSha == \$sha and .conclusion == \"success\")" target/release-chef-runs.jsonl'
```

Expected: product packages/releases are byte-for-byte unchanged, and the two
rehearsal plus one seed run objects all contain the saved SHA and success
conclusion.

### Task 7: Add a failing CI cache-policy contract

**Files:**
- Create: `scripts/check-ci-cache.sh`
- Test: `scripts/check-ci-cache.sh`

- [ ] **Step 1: Create the complete CI policy checker**

```sh
#!/usr/bin/env sh
set -eu

workflow=".github/workflows/ci.yml"

if [ ! -f "$workflow" ]; then
  echo "missing $workflow" >&2
  exit 1
fi

require_count() {
  expected=$1
  needle=$2
  actual=$(grep -Fc "$needle" "$workflow" || true)
  if [ "$actual" -ne "$expected" ]; then
    echo "$workflow expected $expected occurrences of $needle, found $actual" >&2
    exit 1
  fi
}

for directive in \
  "workflow_dispatch:" \
  "use_cache:" \
  "measurement_id:" \
  "dispatch_nonce:" \
  "expected_sha:" \
  "  validate:" \
  "concurrency:" \
  "cancel-in-progress: true" \
  'EXPECTED_SHA: ${{ inputs.expected_sha }}' \
  'if [ "$GITHUB_SHA" != "$EXPECTED_SHA" ]; then'; do
  if ! grep -Fq "$directive" "$workflow"; then
    echo "$workflow is missing $directive" >&2
    exit 1
  fi
done

require_count 2 "needs: validate"
require_count 1 "uses: Swatinem/rust-cache@v2.9.1"
require_count 1 "id: rust-cache"
require_count 1 'rust_cache_hit=${{ steps.rust-cache.outputs.cache-hit }}'
require_count 1 "shared-key: ci"
require_count 1 'save-if: ${{ github.ref == '\''refs/heads/main'\'' && github.event_name != '\''pull_request'\'' }}'

if ! grep -Fq "github.event_name != 'pull_request'" "$workflow"; then
  echo "$workflow does not restrict cache saves to trusted runs" >&2
  exit 1
fi

if ! grep -Fq 'group: ci-${{ github.workflow }}-${{ github.event_name }}-' "$workflow" \
  || ! grep -Fq "inputs.measurement_id" "$workflow" \
  || ! grep -Fq "sha={3}" "$workflow"; then
  echo "$workflow does not isolate manual measurement concurrency" >&2
  exit 1
fi

if grep -Fq "uses: actions/cache@" "$workflow"; then
  echo "$workflow retains the obsolete generic cache action" >&2
  exit 1
fi

echo "CI validates dispatch SHA, restores one compiler-aware cache, and cancels superseded runs"
```

- [ ] **Step 2: Run the checker and verify it fails against current CI**

Run:

```bash
rtk sh scripts/check-ci-cache.sh
```

Expected: FAIL with `.github/workflows/ci.yml is missing workflow_dispatch:`.

- [ ] **Step 3: Verify checker syntax**

Run:

```bash
rtk sh -n scripts/check-ci-cache.sh
```

Expected: exit 0.

### Task 8: Add SHA-bound CI measurement, compiler-aware caching, and cancellation

**Files:**
- Modify: `.github/workflows/ci.yml:1-48`
- Test: `scripts/check-ci-cache.sh`

- [ ] **Step 1: Add the manual cache-measurement input and readable run name**

Use:

```yaml
name: CI
run-name: >-
  ${{ github.event_name == 'workflow_dispatch' && format('CI dispatch={0} cache={1} batch={2} sha={3}', inputs.dispatch_nonce, inputs.use_cache, inputs.measurement_id, inputs.expected_sha) || format('CI ref={0}', github.ref_name) }}

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]
  workflow_dispatch:
    inputs:
      use_cache:
        description: "Restore and save the Rust cache"
        required: true
        default: true
        type: boolean
      measurement_id:
        description: "Unique label for one sequential measurement batch"
        required: false
        default: "manual"
        type: string
      dispatch_nonce:
        description: "Unique selector for this dispatch"
        required: true
        type: string
      expected_sha:
        description: "Exact 40-character main SHA expected by this dispatch"
        required: true
        type: string
```

- [ ] **Step 2: Add cancellation without cancelling a completed measurement run**

Insert:

```yaml
concurrency:
  group: ci-${{ github.workflow }}-${{ github.event_name }}-${{ github.event_name == 'workflow_dispatch' && inputs.measurement_id || github.event.pull_request.number || github.ref }}
  cancel-in-progress: true
```

Push, pull-request, and manual groups can no longer cancel one another. Task 9
uses one unique `measurement_id` and dispatches sequentially; a completed run
is not affected when the next run enters that batch's group.

- [ ] **Step 3: Add a dispatch preflight required by both jobs**

Insert before `build`, and add `needs: validate` once to `build` and once to
`fmt`:

```yaml
  validate:
    name: Validate CI dispatch
    runs-on: ubuntu-latest
    steps:
      - name: Validate measurement inputs
        if: github.event_name == 'workflow_dispatch'
        env:
          EXPECTED_SHA: ${{ inputs.expected_sha }}
          DISPATCH_NONCE: ${{ inputs.dispatch_nonce }}
          MEASUREMENT_ID: ${{ inputs.measurement_id }}
        run: |
          if [ "$GITHUB_REF" != "refs/heads/main" ]; then
            echo "manual measurements must run from main" >&2
            exit 1
          fi
          case "$EXPECTED_SHA" in ''|*[!0-9a-f]*) exit 1 ;; esac
          [ "${#EXPECTED_SHA}" -eq 40 ]
          if [ "$GITHUB_SHA" != "$EXPECTED_SHA" ]; then
            echo "dispatch resolved $GITHUB_SHA instead of $EXPECTED_SHA" >&2
            exit 1
          fi
          case "$DISPATCH_NONCE" in ''|*[!a-z0-9-]*) exit 1 ;; esac
          case "$MEASUREMENT_ID" in ''|*[!a-z0-9-]*) exit 1 ;; esac
```

Push and pull-request runs pass through the empty validation job. Manual runs
cannot consume or save cache at a SHA other than the one selected by the exact
dispatch helper.

- [ ] **Step 4: Replace the commented cache block with rust-cache 2.9.1**

Place this step after Rust installation in the `build` job:

```yaml
      - name: Restore Rust cache
        id: rust-cache
        if: github.event_name != 'workflow_dispatch' || inputs.use_cache
        uses: Swatinem/rust-cache@v2.9.1
        with:
          shared-key: ci
          save-if: ${{ github.ref == 'refs/heads/main' && github.event_name != 'pull_request' }}

      - name: Report Rust cache result
        if: github.event_name != 'workflow_dispatch' || inputs.use_cache
        run: echo "rust_cache_hit=${{ steps.rust-cache.outputs.cache-hit }}"
```

Delete the old commented `actions/cache` block completely.

- [ ] **Step 5: Run the CI policy contract**

```bash
rtk sh scripts/check-ci-cache.sh
rtk actionlint .github/workflows/ci.yml
```

Expected: the checker prints its PASS message and Actionlint exits 0.

- [ ] **Step 6: Run local Rust validation**

```bash
rtk cargo build --workspace
rtk cargo test --workspace
```

Expected: build succeeds and all workspace tests pass.

- [ ] **Step 7: Commit checkpoint 3 implementation**

```bash
rtk git add .github/workflows/ci.yml scripts/check-ci-cache.sh
rtk git commit -m "ci: restore compiler-aware Rust caching"
```

### Task 9: Measure five cold and five warm CI runs

**Files:**
- Verify: `.github/workflows/ci.yml`
- Record results: checkpoint PR verification section

Run after Task 8 merges to `main` so every manual run uses the committed workflow.

- [ ] **Step 1: Generate the measurement batch and SHA exactly once**

```bash
rtk proxy sh -c 'set -eu; rtk mkdir -p target; batch="p1-ci-$(rtk date +%s)-$$"; expected_sha=$(rtk gh api repos/intezya/komodo/commits/main --jq .sha); rtk printf "%s\n" "$batch" > target/ci-measurement-id.txt; rtk printf "%s\n" "$expected_sha" > target/ci-measurement-sha.txt; rtk truncate -s 0 target/ci-cold-run-ids.txt; rtk truncate -s 0 target/ci-warm-run-ids.txt'
```

Expected: one unique measurement ID and one immutable main SHA are persisted.
Every cold, warm-up, and measured warm dispatch below reads these files. If
`main` moves and a dispatch is rejected, discard the whole batch and restart;
never mix SHAs in one sample.

- [ ] **Step 2: Run five cache-disabled jobs sequentially**

Run:

```bash
rtk proxy sh -c 'set -eu; batch=$(rtk sed -n "1p" target/ci-measurement-id.txt); expected_sha=$(rtk sed -n "1p" target/ci-measurement-sha.txt); for run in 1 2 3 4 5; do nonce="p1-ci-cold-$run-$(rtk date +%s)-$$"; title="CI dispatch=$nonce cache=false batch=$batch sha=$expected_sha"; id=$(rtk sh scripts/dispatch-workflow-run.sh intezya/komodo ci.yml main "$title" "$expected_sha" -f use_cache=false -f measurement_id="$batch" -f dispatch_nonce="$nonce" -f expected_sha="$expected_sha"); rtk printf "%s\n" "$id" >> target/ci-cold-run-ids.txt; rtk gh run watch --repo intezya/komodo "$id" --exit-status; done; test "$(rtk sort -u target/ci-cold-run-ids.txt | rtk wc -l | rtk tr -d " ")" = 5'
```

Expected: five successful cache-disabled runs and exactly five unique numeric
IDs in `target/ci-cold-run-ids.txt`.

- [ ] **Step 3: Run one cache warm-up and exclude it from the sample**

```bash
rtk proxy sh -c 'set -eu; batch=$(rtk sed -n "1p" target/ci-measurement-id.txt); expected_sha=$(rtk sed -n "1p" target/ci-measurement-sha.txt); nonce="p1-ci-warmup-$(rtk date +%s)-$$"; title="CI dispatch=$nonce cache=true batch=$batch sha=$expected_sha"; id=$(rtk sh scripts/dispatch-workflow-run.sh intezya/komodo ci.yml main "$title" "$expected_sha" -f use_cache=true -f measurement_id="$batch" -f dispatch_nonce="$nonce" -f expected_sha="$expected_sha"); rtk printf "%s\n" "$id" > target/ci-warmup-run-id.txt; rtk gh run watch --repo intezya/komodo "$id" --exit-status; rtk proxy gh run view --repo intezya/komodo "$id" --log > target/ci-warmup.log'
```

Expected: the successful warm-up populates the compiler-aware cache. Its ID is
saved separately and must not appear in either five-run measurement file.

- [ ] **Step 4: Run five measured warm-cache jobs sequentially and require five hits**

Run:

```bash
rtk proxy sh -c 'set -eu; batch=$(rtk sed -n "1p" target/ci-measurement-id.txt); expected_sha=$(rtk sed -n "1p" target/ci-measurement-sha.txt); for run in 1 2 3 4 5; do nonce="p1-ci-warm-$run-$(rtk date +%s)-$$"; title="CI dispatch=$nonce cache=true batch=$batch sha=$expected_sha"; id=$(rtk sh scripts/dispatch-workflow-run.sh intezya/komodo ci.yml main "$title" "$expected_sha" -f use_cache=true -f measurement_id="$batch" -f dispatch_nonce="$nonce" -f expected_sha="$expected_sha"); rtk printf "%s\n" "$id" >> target/ci-warm-run-ids.txt; rtk gh run watch --repo intezya/komodo "$id" --exit-status; rtk proxy gh run view --repo intezya/komodo "$id" --log > "target/ci-warm-$id.log"; done; test "$(rtk sort -u target/ci-warm-run-ids.txt | rtk wc -l | rtk tr -d " ")" = 5; warmup=$(rtk sed -n "1p" target/ci-warmup-run-id.txt); ! rtk rg -Fxq "$warmup" target/ci-cold-run-ids.txt target/ci-warm-run-ids.txt; while IFS= read -r id; do rtk rg -Fq "rust_cache_hit=true" "target/ci-warm-$id.log"; done < target/ci-warm-run-ids.txt'
```

Expected: five successful measured warm runs, five unique IDs, and an explicit
restore hit in each individual saved log. One match across a glob is not
accepted.

- [ ] **Step 5: Export the ten-run timing evidence**

Run:

```bash
rtk proxy sh -c 'set -eu; for mode in cold warm; do while IFS= read -r id; do rtk gh api "repos/intezya/komodo/actions/runs/$id" | rtk jq --arg mode "$mode" '\''{mode:$mode,databaseId:.id,headSha:.head_sha,displayTitle:.display_title,conclusion,createdAt:.created_at,startedAt:.run_started_at,updatedAt:.updated_at,url:.html_url}'\''; done < "target/ci-$mode-run-ids.txt"; done' | rtk jq -s '.' | rtk tee target/ci-cache-runs.json
```

Run:

```bash
rtk jq -e 'if length != 10 or ([.[].databaseId] | unique | length) != 10 or any(.[]; .conclusion != "success") then error("expected ten unique successful runs") else . end' target/ci-cache-runs.json
```

Expected: exactly ten unique successful objects whose IDs came from the two
captured files. Save this JSON with the checkpoint PR verification evidence.

- [ ] **Step 6: Calculate and save both medians**

Run:

```bash
rtk jq -e 'group_by(.mode) | map({mode: .[0].mode, durations_s: (map((.updatedAt | fromdateiso8601) - (.startedAt | fromdateiso8601)) | sort)}) | if length != 2 or any(.[]; (.durations_s | length) != 5) then error("expected exactly five cold and five warm runs") else map(. + {sample_count: 5, median_s: .durations_s[2]}) end | (map(select(.mode == "cold"))[0]) as $cold | (map(select(.mode == "warm"))[0]) as $warm | if $cold == null or $warm == null or $warm.median_s > ($cold.median_s * 0.75) then error("warm median is not at least 25 percent lower") else {cold:$cold,warm:$warm} end' target/ci-cache-runs.json | rtk proxy tee target/ci-cache-medians.json
```

Expected: two objects with `sample_count: 5`; warm median is at least 25%
lower than cold. If either group is absent, the output has fewer than two
objects and the gate fails rather than silently using extra historical runs.

- [ ] **Step 7: Calculate and save cache-step share keyed by exact run ID**

Run:

```bash
rtk proxy sh -c 'set -eu; while IFS= read -r id; do rtk gh api "repos/intezya/komodo/actions/runs/$id/jobs" | rtk jq -c --argjson run_id "$id" '\''[.jobs[] | select(.name == "Build & Test")] | if length != 1 then error("expected one Build & Test job") else .[0] | {run_id:$run_id,job_s:((.completed_at | fromdateiso8601) - (.started_at | fromdateiso8601)),cache_s:([.steps[] | select(.name | contains("Restore Rust cache")) | ((.completed_at | fromdateiso8601) - (.started_at | fromdateiso8601))] | add)} end'\''; done < target/ci-warm-run-ids.txt' | rtk jq -e -s 'if length != 5 or ([.[].run_id] | unique | length) != 5 or any(.[]; .cache_s == null or .job_s <= 0) then error("missing exact rust-cache timing") else map(. + {cache_pct:(.cache_s / .job_s * 100)}) | if any(.[]; .cache_pct >= 20) then error("cache overhead reached 20 percent") else . end end' | rtk proxy tee target/ci-cache-overhead.json
rtk proxy sh -c 'set -eu; expected=$(rtk jq -Rsc "split(\"\\n\") | map(select(length > 0) | tonumber) | sort" target/ci-warm-run-ids.txt); actual=$(rtk jq -c "map(.run_id) | sort" target/ci-cache-overhead.json); test "$actual" = "$expected"'
```

Expected: the saved file contains five unique `run_id`/`cache_pct` objects,
the IDs exactly equal the measured warm ID file, and every percentage is below
20. Because the step-name filter also matches `Post Restore Rust cache`,
`cache_s` includes restore plus post-job save/upload when GitHub reports both.

### Task 10: Close the dependency finding with an executable no-change audit

**Files:**
- Create: `docs/performance/build-release-dependency-audit.md`
- Verify: `Cargo.toml`
- Verify: `Cargo.lock`

- [ ] **Step 1: Run cargo-machete against only real workspace members**

Install the tool if `rtk cargo machete --version` is unavailable, then run:

```bash
rtk cargo machete --with-metadata bin/core bin/periphery bin/cli lib/command lib/database lib/encoding lib/environment lib/formatting lib/git lib/interpolate lib/transport client/core/rs client/periphery/rs xtask
```

Do not run against repository `.`: the two standalone `example/*` manifests
are intentionally outside `[workspace.members]` and make `cargo metadata`
report a workspace-membership error. Expected: no direct unused dependency is
reported for any real workspace member. Do not use `--fix` on unreviewed
findings.

- [ ] **Step 2: Prove why the Cicada closure is not removable**

Run:

```bash
rtk cargo tree -e features -i cicada_loader --locked
rtk cargo metadata --locked --format-version 1 | rtk jq '.packages[] | select(.name == "mogh_config") | {version,features,manifest_path}'
rtk proxy sh -c 'set -eu; manifest=$(rtk cargo metadata --locked --format-version 1 | rtk jq -r '\''.packages[] | select(.name == "mogh_config") | .manifest_path'\''); crate_dir=$(rtk dirname "$manifest"); rtk rg -n "cicada:|feature = \"cicada\"" "$crate_dir/src" "$manifest"'
rtk rg -n 'ConfigLoader|config_paths|CONFIG_PATHS' bin/core/src/config.rs bin/periphery/src/config.rs bin/cli/src/config.rs client/core/rs/src/entities/config/core.rs client/periphery/rs/src/entities/config.rs
rtk cargo metadata --locked --format-version 1 | rtk jq '.packages | length'
```

Expected: `mogh_config`'s default `cicada` feature reaches
`cicada_loader`; its loader explicitly recognizes `cicada:` paths; and Core,
Periphery, and CLI feed operator-controlled config paths into `ConfigLoader`.
The final count is recorded as graph size, not used as evidence that the
feature is unused.

- [ ] **Step 3: Write the dependency decision record**

Create `docs/performance/build-release-dependency-audit.md` with:

```markdown
# Build, release, and dependency audit

## Direct dependency scan

- Tool/version: cargo-machete <record exact version>
- Scope: the 14 real workspace members, excluding standalone examples
- Result: no unused direct dependency findings

## Cicada decision

- `mogh_config` enables `cicada` by default and therefore resolves
  `cicada_loader` / `cicada_client`.
- This is retained: Core, Periphery, and CLI accept operator-controlled config
  paths, and `mogh_config` treats `cicada:` as a supported remote path scheme.
- Setting `default-features = false` would silently reinterpret or reject an
  existing runtime input. That is a compatibility migration, not a safe P1
  build optimization.
- No `Cargo.toml` or `Cargo.lock` change is authorized by this finding.

## Performance evidence

Record:

- both paired release rehearsal batches and the exact seed run;
- all five before/after stable-cache records and compressed OCI layer bytes;
- the two unique cache-package bootstrap image IDs and their deletion/absence
  proof, with product packages/releases unchanged;
- the four cargo-chef probe statuses/timings, including dependency-manifest
  and lockfile invalidation;
- cold and warm loaded-image byte sizes for binaries, UI, Core, Periphery, and
  CLI plus exact CLI/Periphery `--version` output and Core `GET /version` body,
  all equal to the workspace version;
- five cold/five measured-warm CI run URLs, the excluded warm-up ID,
  cold/warm medians, and all five run-ID-keyed cache percentages.
```

- [ ] **Step 4: Commit only the audited decision**

```bash
rtk git diff --exit-code -- Cargo.toml Cargo.lock
rtk git add docs/performance/build-release-dependency-audit.md
rtk git commit -m "docs: record dependency performance audit"
```

Expected: the commit documents a verified false-positive optimization and
does not alter runtime config semantics or the lockfile.

### Task 11: Run the complete local delivery verification

**Files:**
- Verify: `.github/workflows/release.yml`
- Verify: `.github/workflows/ci.yml`
- Verify: `bin/binaries.Dockerfile.dockerignore`
- Verify: `ui/Dockerfile.dockerignore`
- Verify: `bin/{core,periphery,cli}/single-arch.Dockerfile.dockerignore`
- Verify: `bin/binaries.Dockerfile`
- Verify: `scripts/check-release-targets.sh`
- Verify: `scripts/check-ci-cache.sh`
- Verify: `scripts/dispatch-workflow-run.sh`
- Verify: `scripts/snapshot-release-state.sh`
- Verify: `scripts/inspect-registry-cache.sh`
- Verify: `scripts/delete-seed-bootstrap.sh`
- Verify: `scripts/smoke-release-images.sh`
- Verify: `scripts/probe-cargo-chef.sh`

- [ ] **Step 1: Run executable workflow and shell contracts**

```bash
rtk sh -n scripts/check-release-targets.sh
rtk sh -n scripts/check-ci-cache.sh
rtk sh -n scripts/dispatch-workflow-run.sh
rtk sh -n scripts/snapshot-release-state.sh
rtk sh -n scripts/inspect-registry-cache.sh
rtk sh -n scripts/delete-seed-bootstrap.sh
rtk sh -n scripts/smoke-release-images.sh
rtk sh -n scripts/probe-cargo-chef.sh
rtk sh scripts/check-release-targets.sh
rtk sh scripts/check-ci-cache.sh
rtk actionlint -ignore 'unexpected key "queue"' .github/workflows/release.yml .github/workflows/ci.yml
```

Expected: every syntax/contract check passes. These checks establish the
declared workflow structure and exact static cache expressions. They do not
replace Task 6's remote ref/write proof or Task 9's measured hit proof.

- [ ] **Step 2: Run repository Rust gates**

```bash
rtk cargo fmt --all -- --check
rtk cargo build --workspace
rtk cargo test --workspace
```

Expected: format, build, and tests pass.

- [ ] **Step 3: Repeat the isolated four-probe and cold/warm five-image proof**

```bash
rtk sh scripts/probe-cargo-chef.sh target/final-cargo-chef-probe
rtk awk -F '\t' 'NR == 1 || $1 == "cold" || $1 == "source" || $1 == "dependency" || $1 == "lock"' target/final-cargo-chef-probe/cargo-timings.tsv
rtk awk -F '\t' 'NF == 4 && $4 ~ /^[0-9]+$/ && $4 > 0 {count++} END {exit count != 5}' target/final-cargo-chef-probe/cold-image-sizes.tsv
rtk awk -F '\t' 'NF == 4 && $4 ~ /^[0-9]+$/ && $4 > 0 {count++} END {exit count != 5}' target/final-cargo-chef-probe/source-image-sizes.tsv
rtk cmp target/final-cargo-chef-probe/cold-versions.tsv target/final-cargo-chef-probe/source-versions.tsv
```

Expected: one newly isolated builder covers the cold, source, dependency, and
lock probes. Cold/dependency/lock chef work executes; source chef is cached;
all application builds execute. The helper then repeats the `NO_CACHE=true`
all-five smoke and warm all-five smoke using the isolated builder for UI and
the daemon builder for local-base final images, including exact equal
CLI/Periphery/Core versions, Core HTTP 200, revision labels, and image-size
records. `cold-versions.tsv` and `source-versions.tsv` must compare byte-for-byte.

### Task 12: Freeze evidence, rollout, and rollback

**Files:**
- Modify: `docs/performance/build-release-dependency-audit.md`

- [ ] **Step 1: Attach exact remote evidence**

Copy into the audit document:

- both Task 3 and all three Task 6 exact run IDs/URLs;
- inventory for each disposable ref and all five stable refs before/after seed;
- the two absent-before bootstrap refs, their exact matrix log references,
  deleted single-tag version IDs, and fresh absent-after proof;
- the exact `cargo chef cook ... CACHED` vertex from the warm log;
- all four local cargo-chef timings and cold/warm all-five image sizes;
- all ten CI IDs/URLs, conclusions, cold/warm medians, and all five
  run-ID-keyed `cache_pct` values, plus the excluded warm-up ID;
- the Task 11 exact cold/warm CLI, Periphery, and Core version evidence plus
  Core HTTP-200 result.

Do not substitute `gh run list` output or one generic `CACHED` line.

- [ ] **Step 2: Re-run hard evidence gates and commit**

```bash
rtk jq -e 'length == 10 and ([.[].databaseId] | unique | length) == 10 and all(.[]; .conclusion == "success")' target/ci-cache-runs.json
rtk proxy sh -c 'set -eu; expected_sha=$(rtk sed -n "1p" target/ci-measurement-sha.txt); rtk jq -e --arg sha "$expected_sha" "all(.[]; .headSha == \$sha)" target/ci-cache-runs.json; warmup=$(rtk sed -n "1p" target/ci-warmup-run-id.txt); ! rtk rg -Fxq "$warmup" target/ci-cold-run-ids.txt target/ci-warm-run-ids.txt'
rtk jq -e 'group_by(.mode) | map({mode:.[0].mode,durations_s:(map((.updatedAt | fromdateiso8601) - (.startedAt | fromdateiso8601)) | sort)}) | if length != 2 or any(.[]; (.durations_s | length) != 5) then error("expected five per mode") else map(. + {sample_count:5,median_s:.durations_s[2]}) end | (map(select(.mode == "cold"))[0]) as $cold | (map(select(.mode == "warm"))[0]) as $warm | if $warm.median_s > ($cold.median_s * 0.75) then error("median gate failed") else {cold:$cold,warm:$warm} end' target/ci-cache-runs.json | rtk proxy tee target/ci-cache-medians-repeat.json
rtk cmp target/ci-cache-medians.json target/ci-cache-medians-repeat.json
rtk jq -e 'length == 5 and ([.[].run_id] | unique | length) == 5 and all(.[]; .cache_s != null and .job_s > 0 and .cache_pct < 20)' target/ci-cache-overhead.json
rtk proxy sh -c 'set -eu; expected=$(rtk jq -Rsc "split(\"\\n\") | map(select(length > 0) | tonumber) | sort" target/ci-warm-run-ids.txt); actual=$(rtk jq -c "map(.run_id) | sort" target/ci-cache-overhead.json); test "$actual" = "$expected"; while IFS= read -r id; do rtk rg -Fq "rust_cache_hit=true" "target/ci-warm-$id.log"; done < target/ci-warm-run-ids.txt'
rtk jq -e -n --slurpfile before target/release-stable-before-seed.json --slurpfile after target/release-stable-after-seed.json '$after[0] as $a | $before[0] as $b | ($a | length) == 5 and all($a[]; .absent == false and .layer_bytes > 0) and all($a[]; . as $new | ($b | map(select(.tag == $new.tag))[0]) as $old | ($old.absent == true or $new.id != $old.id or $new.updated_at != $old.updated_at or $new.layer_bytes != $old.layer_bytes))'
rtk jq -e 'length == 2 and all(.[]; .absent_before == false and .deleted_id != null)' target/release-seed-bootstrap-cleanup.json
rtk jq -e 'length == 2 and all(.[]; .absent == true)' target/release-seed-bootstrap-after.json
rtk jq -e -n --slurpfile before target/release-seed-cache-versions-before.json --slurpfile clean target/release-seed-cache-versions-clean.json '($before[0] | map(.id)) as $old | [$clean[0][] | select((.id as $id | $old | index($id)) == null)] | all(.[]; (.tags | length) > 0 and all(.tags[]; . == "binaries" or . == "ui" or . == "komodo-core" or . == "komodo-periphery" or . == "komodo-cli"))'
rtk proxy sh -c 'set -eu; nonce=$(rtk sed -n "1p" target/release-chef-seed-nonce.txt); binaries="seed-binaries-$nonce"; ui="seed-ui-$nonce"; rtk jq -e --arg binaries "$binaries" --arg ui "$ui" "length == 2 and all(.[]; (.tags | length) == 1) and (([\$binaries,\$ui] - (map(.tags[]) | unique)) | length) == 0" target/release-seed-bootstrap-present.json; for image in core periphery cli; do rtk rg -F "ghcr.io/intezya/komodo-build-cache:$binaries" "target/release-chef-seed-$image.log"; done; rtk rg -F "ghcr.io/intezya/komodo-build-cache:$ui" target/release-chef-seed-core.log'
rtk cmp target/release-chef-products-before.json target/release-chef-products-after.json
rtk jq -e -n --slurpfile before target/release-cache-versions-before.json --slurpfile clean target/release-cache-versions-clean.json '($before[0] | map(.id) | sort) == ($clean[0] | map(.id) | sort)'
rtk jq -e -n --slurpfile before target/release-chef-cache-versions-before.json --slurpfile clean target/release-chef-cache-versions-clean.json '($before[0] | map(.id) | sort) == ($clean[0] | map(.id) | sort)'
rtk jq -e 'length == 4 and ([.[].id] | unique | length) == 4' target/release-cache-new-versions.json
rtk jq -e 'length == 4 and ([.[].id] | unique | length) == 4' target/release-chef-new-versions.json
rtk proxy sh -c 'set -eu; sha=$(rtk sed -n "1p" target/release-rehearsal-sha.txt); rtk jq -se --arg sha "$sha" "length == 2 and all(.[]; .headSha == \$sha and .conclusion == \"success\")" target/release-rehearsal-runs.jsonl; sha=$(rtk sed -n "1p" target/release-chef-sha.txt); rtk jq -se --arg sha "$sha" "length == 3 and all(.[]; .headSha == \$sha and .conclusion == \"success\")" target/release-chef-runs.jsonl'
rtk rg -U 'cargo chef cook(.|\n){0,800}CACHED' target/release-chef-second-binaries.log
rtk awk -F '\t' '$1 == "source" {source=($2 == "CACHED" && $4 == "DONE")} $1 == "dependency" {dependency=($2 == "DONE")} $1 == "lock" {lock=($2 == "DONE")} END {exit !(source && dependency && lock)}' target/final-cargo-chef-probe/cargo-timings.tsv
rtk awk -F '\t' 'NF == 4 && $4 ~ /^[0-9]+$/ && $4 > 0 {count++} END {exit count != 5}' target/final-cargo-chef-probe/cold-image-sizes.tsv
rtk awk -F '\t' 'NF == 4 && $4 ~ /^[0-9]+$/ && $4 > 0 {count++} END {exit count != 5}' target/final-cargo-chef-probe/source-image-sizes.tsv
rtk git diff --check
rtk git add docs/performance/build-release-dependency-audit.md
rtk git commit -m "docs: record delivery performance evidence"
```

Expected: evidence is complete and no raw logs, credentials, cache manifests,
or target artifacts are committed.

## Rollback order

1. If a release cache is suspect, stop manual seed/rehearsal dispatches,
   inventory the exact single-tag package version, delete only that version,
   and use the verified isolated `NO_CACHE=true` path. Release tags never write
   stable refs. Before retrying any failed seed, run
   `rtk sh scripts/delete-seed-bootstrap.sh target/rollback-seed-cleanup.json
   "seed-binaries-$nonce" "seed-ui-$nonce"` with the saved nonce, then use
   `inspect-registry-cache.sh` to prove both tags absent.
2. If CI cache restore is unstable, dispatch with `use_cache=false` and revert
   only the rust-cache commit; release images are independent.
3. If cargo-chef changes a binary, compare identical cold/warm five-image
   smoke results, then revert checkpoint 2. Cache refs can be deleted
   independently.
4. There is no Cicada rollback: the audited runtime feature is intentionally
   retained and checkpoint 4 changes documentation only.

## Execution handoff

Plan complete and saved to
`docs/superpowers/plans/2026-07-10-komodo-build-release-dependency-performance.md`.
Execute with `superpowers:subagent-driven-development` for a fresh worker and
two-stage review per task, or `superpowers:executing-plans` for checkpoint
batches in one session. This plan is independent and may run in parallel with
product-runtime checkpoints.
