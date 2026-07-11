#!/usr/bin/env sh
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
context_name=${P1_DOCKER_CONTEXT:-$(docker context show)}
unset BUILDX_BUILDER

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

probe() {
  name=$1
  ignore=$2
  context=$tmp/$name-context
  output=$tmp/$name-output

  mkdir -p \
    "$context/lib/nested/target" \
    "$context/lib/nested/node_modules" \
    "$context/lib/nested/dist" \
    "$context/.dev" \
    "$context/.git" \
    "$context/.worktrees" \
    "$context/target" \
    "$context/ui"

  printf '%s\n' '[workspace]' >"$context/Cargo.toml"
  printf '%s\n' safe >"$context/lib/p1-safe"
  for path in \
    .dev/p1-secret \
    .git/p1-secret \
    .worktrees/p1-secret \
    target/p1-secret \
    lib/nested/target/p1-secret \
    lib/nested/node_modules/p1-secret \
    lib/nested/dist/p1-secret \
    lib/nested/.env \
    lib/nested/.envrc \
    lib/nested/.npmrc \
    lib/nested/.yarnrc.yml \
    ui/.env.development \
    ui/.npmrc
  do
    printf '%s\n' secret >"$context/$path"
  done

  printf '%s\n' 'FROM scratch' 'COPY . /context' \
    >"$context/probe.Dockerfile"
  cp "$repo/$ignore" "$context/probe.Dockerfile.dockerignore"

  docker --context "$context_name" buildx build \
    --builder "$context_name" \
    --file "$context/probe.Dockerfile" \
    --output "type=local,dest=$output" \
    "$context" >/dev/null

  [ -f "$output/context/Cargo.toml" ] ||
    fail "$name context excluded Cargo.toml"
  [ -f "$output/context/lib/p1-safe" ] ||
    fail "$name context excluded lib/p1-safe"

  for path in \
    .dev/p1-secret \
    .git/p1-secret \
    .worktrees/p1-secret \
    target/p1-secret \
    lib/nested/target/p1-secret \
    lib/nested/node_modules/p1-secret \
    lib/nested/dist/p1-secret \
    lib/nested/.env \
    lib/nested/.envrc \
    lib/nested/.npmrc \
    lib/nested/.yarnrc.yml \
    ui/.env.development \
    ui/.npmrc
  do
    [ ! -e "$output/context/$path" ] ||
      fail "$name context included denied path: $path"
  done
}

probe core bin/core/aio.Dockerfile.dockerignore
probe periphery bin/periphery/aio.Dockerfile.dockerignore

printf '%s\n' 'P1 local BuildKit context tests OK'
