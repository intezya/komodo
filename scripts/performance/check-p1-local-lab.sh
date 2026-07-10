#!/usr/bin/env sh
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$repo"

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

for path in \
  p1.local.compose.yaml \
  compose/p1.local.env.example \
  bin/core/aio.Dockerfile.dockerignore \
  bin/periphery/aio.Dockerfile.dockerignore
do
  [ -f "$path" ] || fail "missing $path"
done

expected_env='P1_MONGO_ROOT_USERNAME=komodo-p1
P1_MONGO_ROOT_PASSWORD=example-only-not-for-runtime
P1_DATABASE_NAME=komodo_p1_local
P1_INIT_ADMIN_USERNAME=p1-admin
P1_INIT_ADMIN_PASSWORD=example-only-not-for-runtime
P1_JWT_SECRET=example-only-not-for-runtime
P1_WEBHOOK_SECRET=example-only-not-for-runtime
P1_FIRST_SERVER_NAME=p1-local
P1_DOCKER_SOCKET=/var/run/docker.sock
P1_PERIPHERY_ROOT=/tmp/komodo-p1-local/periphery
P1_MONGO_PORT=27017
P1_CORE_A_PORT=9120
P1_CORE_B_PORT=9121
P1_PERIPHERY_PORT=8120'

[ "$(cat compose/p1.local.env.example)" = "$expected_env" ] ||
  fail 'example environment contract mismatch'

rendered=$(mktemp)
trap 'rm -f "$rendered"' EXIT HUP INT TERM
docker compose \
  --project-name komodo-p1-local \
  --env-file compose/p1.local.env.example \
  --profile cross-core \
  --profile tools \
  --file p1.local.compose.yaml \
  config --format json >"$rendered"

mongo='mongo:8.0.26@sha256:ffa440e8d62533e24a67696ae1bbb46e610ebb3167d65abd122b496ae06d28e6'
jq -e --arg mongo "$mongo" --arg repo "$repo" '
  . as $model |
  def env($service): $model.services[$service].environment;
  def env_keys($service): env($service) | keys;
  def build($service; $dockerfile):
    $model.services[$service].build == {
      context: $repo,
      dockerfile: $dockerfile
    };
  def port_tuples:
    [$model.services[] | .ports[]? |
      [.host_ip, (.published | tonumber), .target]] | sort;
  def volume_sources($service):
    [$model.services[$service].volumes[]? | .source] | sort;
  def dependency_keys($service):
    [$model.services[$service].depends_on | keys[]] | sort;
  def healthy_dependencies($service):
    [$model.services[$service].depends_on[] | .condition] |
      all(. == "service_healthy");
  def core_health($service):
    $model.services[$service].healthcheck == {
      test: ["CMD-SHELL", "curl --fail http://127.0.0.1:9120/version"],
      timeout: "5s",
      interval: "2s",
      retries: 30,
      start_period: "10s"
    };

  (.services | keys | sort) ==
    ["core-a", "core-b", "mongo", "periphery", "toolbox"] and
  ([.services[] | .profiles[]?] | unique | sort) ==
    ["cross-core", "tools"] and

  build("core-a"; "bin/core/aio.Dockerfile") and
  build("core-b"; "bin/core/aio.Dockerfile") and
  build("periphery"; "bin/periphery/aio.Dockerfile") and
  .services.mongo.image == $mongo and
  .services.toolbox.image == $mongo and
  .services["core-a"].image == "komodo-p1-local-core:dev" and
  .services["core-b"].image == "komodo-p1-local-core:dev" and
  .services.periphery.image == "komodo-p1-local-periphery:dev" and
  .services["core-b"].profiles == ["cross-core"] and
  .services.toolbox.profiles == ["tools"] and
  (.services.mongo | has("profiles") | not) and
  (.services["core-a"] | has("profiles") | not) and
  (.services.periphery | has("profiles") | not) and
  env_keys("mongo") == [
    "MONGO_INITDB_ROOT_PASSWORD",
    "MONGO_INITDB_ROOT_USERNAME"
  ] and
  env_keys("core-a") == [
    "KOMODO_BIND_IP",
    "KOMODO_DATABASE_ADDRESS",
    "KOMODO_DATABASE_APP_NAME",
    "KOMODO_DATABASE_DB_NAME",
    "KOMODO_DATABASE_PASSWORD",
    "KOMODO_DATABASE_USERNAME",
    "KOMODO_DISABLE_INIT_RESOURCES",
    "KOMODO_DISABLE_USER_REGISTRATION",
    "KOMODO_FIRST_SERVER_ADDRESS",
    "KOMODO_FIRST_SERVER_NAME",
    "KOMODO_HOST",
    "KOMODO_INIT_ADMIN_PASSWORD",
    "KOMODO_INIT_ADMIN_USERNAME",
    "KOMODO_JWT_SECRET",
    "KOMODO_LOCAL_AUTH",
    "KOMODO_MONITORING_INTERVAL",
    "KOMODO_PORT",
    "KOMODO_PRIVATE_KEY",
    "KOMODO_RESOURCE_POLL_INTERVAL",
    "KOMODO_WEBHOOK_SECRET"
  ] and
  env_keys("core-b") == [
    "KOMODO_BIND_IP",
    "KOMODO_DATABASE_ADDRESS",
    "KOMODO_DATABASE_APP_NAME",
    "KOMODO_DATABASE_DB_NAME",
    "KOMODO_DATABASE_PASSWORD",
    "KOMODO_DATABASE_USERNAME",
    "KOMODO_DISABLE_INIT_RESOURCES",
    "KOMODO_DISABLE_USER_REGISTRATION",
    "KOMODO_HOST",
    "KOMODO_JWT_SECRET",
    "KOMODO_LOCAL_AUTH",
    "KOMODO_MONITORING_INTERVAL",
    "KOMODO_PORT",
    "KOMODO_PRIVATE_KEY",
    "KOMODO_RESOURCE_POLL_INTERVAL",
    "KOMODO_WEBHOOK_SECRET"
  ] and
  env_keys("periphery") == [
    "PERIPHERY_BIND_IP",
    "PERIPHERY_CORE_PUBLIC_KEYS",
    "PERIPHERY_INCLUDE_DISK_MOUNTS",
    "PERIPHERY_PORT",
    "PERIPHERY_PRIVATE_KEY",
    "PERIPHERY_ROOT_DIRECTORY",
    "PERIPHERY_SERVER_ENABLED",
    "PERIPHERY_SSL_ENABLED"
  ] and
  env_keys("toolbox") == [
    "MONGO_INITDB_ROOT_PASSWORD",
    "MONGO_INITDB_ROOT_USERNAME",
    "P1_DATABASE_NAME"
  ] and
  port_tuples == [
    ["127.0.0.1", 8120, 8120],
    ["127.0.0.1", 9120, 9120],
    ["127.0.0.1", 9121, 9120],
    ["127.0.0.1", 27017, 27017]
  ] and
  ([.services[] | (.networks | keys)] | all(. == ["lab"])) and

  volume_sources("mongo") == ["mongo-config", "mongo-data"] and
  (volume_sources("core-a") | index("keys")) != null and
  (volume_sources("core-b") | index("keys")) != null and
  (volume_sources("periphery") | index("keys")) != null and
  ([.services | to_entries[] |
    select(any(.value.volumes[]?;
      .target == "/var/run/docker.sock")) | .key] == ["periphery"]) and
  ([.services | to_entries[] |
    select(any(.value.volumes[]?; .target == "/proc")) | .key] ==
      ["periphery"]) and
  any(.services.periphery.volumes[]?;
    .source == "/proc" and .target == "/proc" and
    .read_only == true) and
  (env("periphery").PERIPHERY_ROOT_DIRECTORY as $root |
    any(.services.periphery.volumes[]?;
      .source == $root and .target == $root)) and

  env("core-a").KOMODO_DATABASE_ADDRESS == "mongo:27017" and
  env("core-a").KOMODO_DATABASE_ADDRESS ==
    env("core-b").KOMODO_DATABASE_ADDRESS and
  env("core-a").KOMODO_DATABASE_USERNAME ==
    env("core-b").KOMODO_DATABASE_USERNAME and
  env("core-a").KOMODO_DATABASE_PASSWORD ==
    env("core-b").KOMODO_DATABASE_PASSWORD and
  env("core-a").KOMODO_DATABASE_DB_NAME ==
    env("core-b").KOMODO_DATABASE_DB_NAME and
  env("core-a").KOMODO_DATABASE_DB_NAME == "komodo_p1_local" and
  env("core-a").KOMODO_PRIVATE_KEY ==
    env("core-b").KOMODO_PRIVATE_KEY and
  env("core-a").KOMODO_JWT_SECRET ==
    env("core-b").KOMODO_JWT_SECRET and
  env("core-a").KOMODO_WEBHOOK_SECRET ==
    env("core-b").KOMODO_WEBHOOK_SECRET and
  env("core-a").KOMODO_HOST == "http://127.0.0.1:9120" and
  env("core-b").KOMODO_HOST == "http://localhost:9121" and
  env("core-a").KOMODO_DATABASE_APP_NAME == "komodo_p1_core_a" and
  env("core-b").KOMODO_DATABASE_APP_NAME == "komodo_p1_core_b" and
  env("core-a").KOMODO_FIRST_SERVER_ADDRESS ==
    "http://periphery:8120" and
  (env("core-a") | has("KOMODO_FIRST_SERVER_NAME")) and
  (env("core-a") | has("KOMODO_INIT_ADMIN_USERNAME")) and
  (env("core-a") | has("KOMODO_INIT_ADMIN_PASSWORD")) and
  (["core-a", "core-b"] | all(. as $service |
    env($service).KOMODO_LOCAL_AUTH == "true" and
    env($service).KOMODO_DISABLE_USER_REGISTRATION == "true" and
    env($service).KOMODO_DISABLE_INIT_RESOURCES == "true" and
    env($service).KOMODO_MONITORING_INTERVAL == "15-sec" and
    env($service).KOMODO_RESOURCE_POLL_INTERVAL == "1-day")) and
  (env("core-b") | has("KOMODO_FIRST_SERVER_NAME") | not) and
  (env("core-b") | has("KOMODO_FIRST_SERVER_ADDRESS") | not) and
  (env("core-b") | has("KOMODO_INIT_ADMIN_USERNAME") | not) and
  (env("core-b") | has("KOMODO_INIT_ADMIN_PASSWORD") | not) and

  env("periphery").PERIPHERY_SERVER_ENABLED == "true" and
  env("periphery").PERIPHERY_PORT == "8120" and
  env("periphery").PERIPHERY_BIND_IP == "0.0.0.0" and
  env("periphery").PERIPHERY_SSL_ENABLED == "false" and
  env("periphery").PERIPHERY_PRIVATE_KEY ==
    "file:/config/keys/periphery.key" and
  env("periphery").PERIPHERY_CORE_PUBLIC_KEYS ==
    "file:/config/keys/core.pub" and
  (env("periphery") | has("PERIPHERY_CORE_ADDRESS") | not) and
  (env("periphery") | has("PERIPHERY_CORE_ADDRESSES") | not) and
  .services["core-b"].restart == "no" and
  .services.toolbox.restart == "no" and

  .services.mongo.healthcheck == {
    test: ["CMD-SHELL",
      "mongosh --quiet --username \"$$MONGO_INITDB_ROOT_USERNAME\" --password \"$$MONGO_INITDB_ROOT_PASSWORD\" --authenticationDatabase admin --eval \u0027db.adminCommand({ ping: 1 }).ok\u0027 | grep 1"],
    timeout: "5s",
    interval: "2s",
    retries: 30,
    start_period: "10s"
  } and
  core_health("core-a") and
  core_health("core-b") and
  .services.periphery.healthcheck == {
    test: ["CMD-SHELL", "curl --fail http://127.0.0.1:8120/version"],
    timeout: "5s",
    interval: "2s",
    retries: 30,
    start_period: "5s"
  } and
  dependency_keys("core-a") == ["mongo"] and
  dependency_keys("periphery") == ["core-a"] and
  dependency_keys("core-b") == ["core-a", "mongo", "periphery"] and
  healthy_dependencies("core-a") and
  healthy_dependencies("periphery") and
  healthy_dependencies("core-b") and

  ([.services[] |
    (.image // empty),
    (.environment // {} | .[])] |
    any(test("ghcr\\.io|ferretdb|mongodb\\+srv|komo\\.do"; "i")) |
    not)
' "$rendered" >/dev/null || fail 'rendered Compose contract mismatch'

core_ignore='*
!Cargo.toml
!Cargo.lock
!lib/
!lib/**
!client/
!client/core/
!client/core/rs/
!client/core/rs/**
!client/core/ts/
!client/core/ts/**
!client/periphery/
!client/periphery/**
!bin/
!bin/core/
!bin/core/**
!bin/cli/
!bin/cli/**
!bin/entrypoint.sh
!xtask/
!xtask/**
!ui/
!ui/**
!config/
!config/core.config.toml
.git
**/.git
**/.git/**
.worktrees
**/.worktrees
**/.worktrees/**
.dev
**/.dev
**/.dev/**
target
**/target
**/target/**
**/node_modules
**/node_modules/**
**/dist
**/dist/**
.env*
**/.env*
.npmrc
**/.npmrc
.yarnrc*
**/.yarnrc*'

periphery_ignore='*
!Cargo.toml
!Cargo.lock
!lib/
!lib/**
!client/
!client/core/
!client/core/rs/
!client/core/rs/**
!client/periphery/
!client/periphery/**
!bin/
!bin/periphery/
!bin/periphery/**
!bin/entrypoint.sh
!xtask/
!xtask/**
.git
**/.git
**/.git/**
.worktrees
**/.worktrees
**/.worktrees/**
.dev
**/.dev
**/.dev/**
target
**/target
**/target/**
**/node_modules
**/node_modules/**
**/dist
**/dist/**
.env*
**/.env*
.npmrc
**/.npmrc
.yarnrc*
**/.yarnrc*'

[ "$(cat bin/core/aio.Dockerfile.dockerignore)" = "$core_ignore" ] ||
  fail 'Core AIO build-context allowlist mismatch'
[ "$(cat bin/periphery/aio.Dockerfile.dockerignore)" = "$periphery_ignore" ] ||
  fail 'Periphery AIO build-context allowlist mismatch'

printf 'P1 local lab contract OK\n'
