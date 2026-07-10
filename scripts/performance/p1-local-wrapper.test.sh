#!/usr/bin/env sh
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
wrapper="$repo/scripts/performance/p1-local.sh"

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

[ -x "$wrapper" ] || {
  printf '%s\n' 'missing scripts/performance/p1-local.sh' >&2
  exit 1
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
fake_bin="$tmp/bin"
system_path=$PATH
mkdir "$fake_bin"

cat >"$fake_bin/docker" <<'EOF'
#!/usr/bin/env sh
set -eu

log=${P1_TEST_DOCKER_LOG:?}
trace=${P1_TEST_TRACE:?}
printf '%s\n' "$*" >>"$log"

case " $* " in
  *" compose "*)
    env | grep -E '^(P1_MONGO_ROOT_USERNAME|P1_MONGO_ROOT_PASSWORD|P1_DATABASE_NAME|P1_INIT_ADMIN_USERNAME|P1_INIT_ADMIN_PASSWORD|P1_JWT_SECRET|P1_WEBHOOK_SECRET|P1_FIRST_SERVER_NAME|P1_DOCKER_SOCKET|P1_PERIPHERY_ROOT|P1_MONGO_PORT|P1_CORE_A_PORT|P1_CORE_B_PORT|P1_PERIPHERY_PORT|COMPOSE_FILE|COMPOSE_PROJECT_NAME|COMPOSE_ENV_FILES|COMPOSE_PROFILES|BUILDX_BUILDER)=' \
      >>"${P1_TEST_ENV_LOG:?}" || true
    ;;
esac

if [ "${1-}" = context ] && [ "${2-}" = show ]; then
  printf '%s\n' test-local
  exit 0
fi

if [ "${1-}" = context ] && [ "${2-}" = inspect ]; then
  printf '%s\n' "${P1_TEST_ENDPOINT:-unix:///tmp/p1-test-docker.sock}"
  exit 0
fi

if [ "${1-}" = --context ]; then
  [ "${2-}" = test-local ] || exit 91
  shift 2
fi

case "${1-}" in
  info)
    [ "${P1_TEST_INFO_FAIL:-0}" = 0 ] || exit 92
    printf '%s\n' arm64
    exit 0
    ;;
  version)
    printf '%s\n' 27.0.0
    exit 0
    ;;
  buildx)
    case " ${*} " in
      *" version "*) printf '%s\n' 'github.com/docker/buildx 0.20.0' ;;
      *" inspect "*)
        printf 'Endpoint: %s\n' "${P1_TEST_BUILDER_ENDPOINT:-test-local}"
        ;;
    esac
    exit 0
    ;;
  run)
    exit 0
    ;;
  volume)
    exit 0
    ;;
  inspect)
    printf '%s\n' healthy
    exit 0
    ;;
  ps)
    [ "${P1_TEST_CORE_B_EXISTS:-0}" = 1 ] && printf '%s\n' core-b-id
    exit 0
    ;;
  compose)
    ;;
  *)
    exit 93
    ;;
esac

args=" $* "

case "$args" in
  *" config --services "*)
    printf '%s\n' core-a core-b mongo periphery toolbox
    exit 0
    ;;
  *" config --profiles "*)
    printf '%s\n' cross-core tools
    exit 0
    ;;
  *" ps -q core-b "*)
    [ "${P1_TEST_CORE_B_EXISTS:-0}" = 1 ] && printf '%s\n' core-b-id
    exit 0
    ;;
  *" ps -q mongo "*) printf '%s\n' mongo-id; exit 0 ;;
  *" ps -q core-a "*) printf '%s\n' core-a-id; exit 0 ;;
  *" ps -q periphery "*) printf '%s\n' periphery-id; exit 0 ;;
  *" ps --all "*)
    printf '%s\n' 'mongo running healthy 127.0.0.1:27017' \
      'core-a running healthy 127.0.0.1:9120' \
      'periphery running healthy 127.0.0.1:8120'
    exit 0
    ;;
  *" pause core-a "*)
    printf '%s\n' pause >>"$trace"
    exit 0
    ;;
  *" unpause core-a "*)
    printf '%s\n' unpause >>"$trace"
    exit 0
    ;;
  *" rm --stop --force core-b "*)
    printf '%s\n' core-b-rm >>"$trace"
    exit 0
    ;;
  *" up -d --no-deps core-b "*)
    printf '%s\n' core-b-up >>"$trace"
    [ "${P1_TEST_CORE_B_START_FAIL:-0}" = 0 ] || exit 94
    exit 0
    ;;
esac

case "$args" in
  *" --profile tools run --rm --no-deps toolbox "*)
    case "$args" in
      *"in_progress_updates"*)
        count_file="${P1_TEST_CASE_DIR:?}/preflight-count"
        count=0
        [ ! -f "$count_file" ] || count=$(cat "$count_file")
        count=$((count + 1))
        printf '%s\n' "$count" >"$count_file"
        printf 'preflight:%s\n' "$count" >>"$trace"

        blocker=${P1_TEST_BLOCKER:-none}
        if [ "${P1_TEST_SECOND_PREFLIGHT_FAIL:-0}" = 1 ] &&
          [ "$count" -eq 2 ]
        then
          blocker=in_progress_updates
        fi
        in_progress_updates=0
        procedure_schedules=0
        action_schedules=0
        startup_actions=0
        gitops_stacks=0
        gitops_resource_syncs=0
        case "$blocker" in
          none) ;;
          in_progress_updates) in_progress_updates=1 ;;
          procedure_schedules) procedure_schedules=1 ;;
          action_schedules) action_schedules=1 ;;
          startup_actions) startup_actions=1 ;;
          gitops_stacks) gitops_stacks=1 ;;
          gitops_resource_syncs) gitops_resource_syncs=1 ;;
          *) exit 95 ;;
        esac
        printf '{"in_progress_updates":%s,"procedure_schedules":%s,"action_schedules":%s,"startup_actions":%s,"gitops_stacks":%s,"gitops_resource_syncs":%s}\n' \
          "$in_progress_updates" "$procedure_schedules" \
          "$action_schedules" "$startup_actions" \
          "$gitops_stacks" "$gitops_resource_syncs"
        [ "$blocker" = none ] && exit 0
        exit 42
        ;;
      *)
        printf '%s\n' 1
        exit 0
        ;;
    esac
    ;;
esac

case "$args" in
  *" build core-a periphery "*) exit 0 ;;
  *" up -d --build mongo core-a "*) exit 0 ;;
  *" up -d --build --no-deps periphery "*) exit 0 ;;
  *" down --remove-orphans "*) exit 0 ;;
  *" down --volumes --remove-orphans "*) exit 0 ;;
  *" compose version "*) printf '%s\n' 'Docker Compose version v2.30.0'; exit 0 ;;
esac

exit 0
EOF

cat >"$fake_bin/curl" <<'EOF'
#!/usr/bin/env sh
set -eu
printf '%s\n' "$*" >>"${P1_TEST_CURL_LOG:?}"
printf 'curl:%s\n' "$*" >>"${P1_TEST_TRACE:?}"
case " $* " in
  *" --config - "*) cat >/dev/null ;;
  *" --data-binary @- "*) cat >/dev/null ;;
esac
case " $* " in
  *"/version"*)
    count_file="${P1_TEST_CASE_DIR:?}/version-count"
    count=0
    [ ! -f "$count_file" ] || count=$(cat "$count_file")
    count=$((count + 1))
    printf '%s\n' "$count" >"$count_file"
    if [ "$count" -le "${P1_TEST_VERSION_FAILURES:-0}" ]; then
      exit 7
    fi
    ;;
esac
case " $* " in
  *"127.0.0.1:9121"*)
    [ "${P1_TEST_CORE_B_READY_FAIL:-0}" = 0 ] || exit 28
    ;;
esac
case " $* " in
  *"/version"*) printf '%s\n' '1.19.4';;
  *"/auth/login/LoginLocalUser"*)
    printf '%s\n' '{"type":"Jwt","data":{"jwt":"p1-test-jwt"}}'
    ;;
  *"/read/GetSystemStats"*)
    if [ "${P1_TEST_INVALID_STATS:-0}" = 1 ]; then
      printf '%s\n' '{"cpu_perc":"invalid","mem_total_gb":0}'
    else
      printf '%s\n' '{"cpu_perc":1.5,"mem_total_gb":8,"polling_rate":"15-sec","refresh_ts":1}'
    fi
    ;;
  *"/read/ListDockerContainers"*)
    if [ "${P1_TEST_WRONG_DAEMON_IDS:-0}" = 1 ]; then
      printf '%s\n' '{"containers":[{"id":"different-daemon-id"}]}'
    else
      printf '%s\n' '{"containers":[{"id":"mongo-id"},{"id":"core-a-id"},{"id":"periphery-id"}]}'
    fi
    ;;
  *"/read/GetCoreInfo"*)
    printf '%s\n' '{"public_key":"p1-test-core-public-key"}'
    ;;
  *) exit 96 ;;
esac
EOF

cat >"$fake_bin/openssl" <<'EOF'
#!/usr/bin/env sh
set -eu
case " $* " in
  *" 24 "*) printf '%s\n' 111111111111111111111111111111111111111111111111 ;;
  *" 32 "*) printf '%s\n' 2222222222222222222222222222222222222222222222222222222222222222 ;;
  *) exit 97 ;;
esac
EOF

cat >"$fake_bin/df" <<'EOF'
#!/usr/bin/env sh
printf '%s\n' 'Filesystem 1024-blocks Used Available Capacity Mounted on' \
  '/dev/test 104857600 1 104857599 1% /'
EOF

cat >"$fake_bin/sleep" <<'EOF'
#!/usr/bin/env sh
exit 0
EOF

chmod +x "$fake_bin/docker" "$fake_bin/curl" \
  "$fake_bin/openssl" "$fake_bin/df" "$fake_bin/sleep"

case_number=0
reset_case() {
  case_number=$((case_number + 1))
  case_dir="$tmp/case-$case_number"
  mkdir "$case_dir"
  : >"$case_dir/docker.log"
  : >"$case_dir/curl.log"
  : >"$case_dir/env.log"
  : >"$case_dir/trace.log"
  out="$case_dir/out"
  state="$case_dir/state"
  export P1_TEST_CASE_DIR="$case_dir"
  export P1_TEST_DOCKER_LOG="$case_dir/docker.log"
  export P1_TEST_CURL_LOG="$case_dir/curl.log"
  export P1_TEST_ENV_LOG="$case_dir/env.log"
  export P1_TEST_TRACE="$case_dir/trace.log"
  export P1_STATE_DIR="$state"
  export P1_DOCKER_CONTEXT=test-local
  unset P1_ENV_FILE P1_PERIPHERY_ROOT P1_DOCKER_SOCKET
  unset P1_TEST_ENDPOINT P1_TEST_INFO_FAIL P1_TEST_BUILDER_ENDPOINT
  unset P1_TEST_CORE_B_EXISTS P1_TEST_BLOCKER
  unset P1_TEST_SECOND_PREFLIGHT_FAIL P1_TEST_CORE_B_START_FAIL
  unset P1_TEST_CORE_B_READY_FAIL
  unset P1_TEST_VERSION_FAILURES P1_TEST_INVALID_STATS
  unset P1_TEST_WRONG_DAEMON_IDS
}

run_wrapper() {
  set +e
  PATH="$fake_bin:$system_path" "$wrapper" "$@" >"$out" 2>&1
  rc=$?
  set -e
}

expect_rc() {
  expected=$1
  [ "$rc" -eq "$expected" ] ||
    fail "expected exit $expected, got $rc: $(cat "$out")"
}

expect_failure() {
  [ "$rc" -ne 0 ] || fail 'expected command failure'
}

assert_output_contains() {
  grep -Fq "$1" "$out" ||
    fail "missing output '$1': $(cat "$out")"
}

assert_no_docker() {
  [ ! -s "$P1_TEST_DOCKER_LOG" ] ||
    fail "unexpected Docker call: $(cat "$P1_TEST_DOCKER_LOG")"
}

write_env() {
  env_file=${1:-$P1_STATE_DIR/p1-local.env}
  mkdir -p "$(dirname -- "$env_file")"
  cat >"$env_file" <<'EOF'
P1_MONGO_ROOT_USERNAME=komodo-p1
P1_MONGO_ROOT_PASSWORD=test-mongo-secret
P1_DATABASE_NAME=komodo_p1_local
P1_INIT_ADMIN_USERNAME=p1-admin
P1_INIT_ADMIN_PASSWORD=test-admin-secret
P1_JWT_SECRET=test-jwt-secret
P1_WEBHOOK_SECRET=test-webhook-secret
P1_FIRST_SERVER_NAME=p1-local
P1_DOCKER_SOCKET=/var/run/docker.sock
P1_PERIPHERY_ROOT=/tmp/p1-test-periphery
P1_MONGO_PORT=27017
P1_CORE_A_PORT=9120
P1_CORE_B_PORT=9121
P1_PERIPHERY_PORT=8120
EOF
  chmod 0600 "$env_file"
  export P1_ENV_FILE="$env_file"
}

assert_compose_prefixes() {
  expected_env_file=$(python3 - "$P1_ENV_FILE" <<'PY'
import os
import sys

path = sys.argv[1]
print(os.path.join(os.path.realpath(os.path.dirname(path)), os.path.basename(path)))
PY
)
  while IFS= read -r line; do
    case " $line " in
      *" compose "*)
        case " $line " in
          *"--context test-local compose --project-name komodo-p1-local --env-file $expected_env_file --file $repo/p1.local.compose.yaml"*) ;;
          *"--context test-local compose --project-name komodo-p1-local --env-file $repo/compose/p1.local.env.example --file $repo/p1.local.compose.yaml version"*) ;;
          *) fail "Compose call escaped fixed prefix: $line" ;;
        esac
        ;;
    esac
  done <"$P1_TEST_DOCKER_LOG"
}

assert_no_secrets_in_argv() {
  if grep -Eq 'test-mongo-secret|test-admin-secret|test-jwt-secret|test-webhook-secret|p1-test-jwt' \
    "$P1_TEST_DOCKER_LOG" "$P1_TEST_CURL_LOG"
  then
    fail 'secret exposed in recorded argv'
  fi
}

reset_case
run_wrapper unknown
expect_rc 64
assert_no_docker

reset_case
run_wrapper doctor extra
expect_rc 64
assert_no_docker

reset_case
run_wrapper reset
expect_rc 64
assert_output_contains 'reset requires --yes'
assert_no_docker

reset_case
run_wrapper reset --force
expect_rc 64
assert_output_contains 'reset requires --yes'
assert_no_docker

reset_case
run_wrapper reset --yes extra
expect_rc 64
assert_output_contains 'reset requires --yes'
assert_no_docker

for endpoint in ssh://host/run/docker.sock tcp://127.0.0.1:2375 relative.sock; do
  reset_case
  export P1_TEST_ENDPOINT=$endpoint
  run_wrapper doctor
  expect_failure
  assert_output_contains 'local unix'
  if grep -Eq ' info | compose | buildx | run ' "$P1_TEST_DOCKER_LOG"; then
    fail "unsafe endpoint reached daemon preflight: $endpoint"
  fi
  [ ! -e "$P1_STATE_DIR" ] || fail 'doctor created persistent state'
done

for command in doctor build up; do
  reset_case
  export P1_TEST_INFO_FAIL=1
  run_wrapper "$command"
  expect_failure
  [ ! -e "$P1_STATE_DIR" ] ||
    fail "$command created state after daemon preflight failure"
  if grep -Eq ' compose .* (build|up) ' "$P1_TEST_DOCKER_LOG"; then
    fail "$command mutated Compose after daemon preflight failure"
  fi
done

for command in build up; do
  reset_case
  export P1_TEST_ENDPOINT=ssh://host/run/docker.sock
  run_wrapper "$command"
  expect_failure
  [ ! -e "$P1_STATE_DIR" ] ||
    fail "$command created state after failed preflight"
  if grep -Eq ' compose .* (build|up) ' "$P1_TEST_DOCKER_LOG"; then
    fail "$command mutated Compose after failed preflight"
  fi
done

reset_case
run_wrapper build
expect_rc 0
generated_env="$P1_STATE_DIR/p1-local.env"
[ -f "$generated_env" ] || fail 'runtime env was not generated'
generated_mode=$(python3 - "$generated_env" <<'PY'
import os
import stat
import sys

print(oct(stat.S_IMODE(os.lstat(sys.argv[1]).st_mode))[2:])
PY
)
[ "$generated_mode" = 600 ] ||
  fail 'runtime env mode is not 0600'
[ "$(wc -l <"$generated_env" | tr -d ' ')" -eq 14 ] ||
  fail 'runtime env key count mismatch'
[ -z "$(find "$P1_STATE_DIR" -name '*.tmp*' -print)" ] ||
  fail 'runtime env temporary file was not atomically renamed'
grep -Fq 'build core-a periphery' "$P1_TEST_DOCKER_LOG" ||
  fail 'build did not select Core and Periphery'
export P1_ENV_FILE=$generated_env
assert_compose_prefixes

reset_case
mkdir -p "$case_dir/external"
valid="$case_dir/external/valid.env"
write_env "$valid"
ln -s "$valid" "$case_dir/external/link.env"
export P1_ENV_FILE="$case_dir/external/link.env"
run_wrapper status
expect_failure
assert_output_contains 'regular'
assert_no_docker

reset_case
mkdir -p "$case_dir/external/directory.env"
export P1_ENV_FILE="$case_dir/external/directory.env"
run_wrapper status
expect_failure
assert_output_contains 'regular'
assert_no_docker

reset_case
bad_mode="$case_dir/bad-mode.env"
write_env "$bad_mode"
chmod 0644 "$bad_mode"
run_wrapper status
expect_failure
assert_output_contains '0600'
assert_no_docker

reset_case
export P1_ENV_FILE=/etc/hosts
run_wrapper status
expect_failure
assert_output_contains 'owned'
assert_no_docker

reset_case
export P1_ENV_FILE="$repo/compose/p1.local.env.example"
run_wrapper status
expect_failure
assert_output_contains 'example'
assert_no_docker

for inside in state env root; do
  reset_case
  case "$inside" in
    state) export P1_STATE_DIR="$repo/.p1-test-state"; run_wrapper doctor ;;
    env)
      inside_env="$repo/target/p1-local-wrapper-test-$$/p1-local.env"
      write_env "$inside_env"
      run_wrapper status
      rm -rf "$repo/target/p1-local-wrapper-test-$$"
      ;;
    root)
      export P1_PERIPHERY_ROOT="$repo/.p1-test-root"
      run_wrapper build
      ;;
  esac
  expect_failure
  assert_no_docker
done

reset_case
export P1_ENV_FILE="$repo/compose/p1.local.env.example"
export P1_MONGO_ROOT_PASSWORD=ambient-mongo-secret
export P1_MONGO_ROOT_USERNAME=ambient-mongo-user
export P1_DATABASE_NAME=ambient-database
export P1_INIT_ADMIN_USERNAME=ambient-admin-user
export P1_INIT_ADMIN_PASSWORD=ambient-admin-secret
export P1_JWT_SECRET=ambient-jwt-secret
export P1_WEBHOOK_SECRET=ambient-webhook-secret
export P1_FIRST_SERVER_NAME=ambient-server
export P1_DOCKER_SOCKET=/ambient/docker.sock
export P1_PERIPHERY_ROOT=/ambient/periphery
export P1_MONGO_PORT=37017
export P1_CORE_A_PORT=39120
export P1_CORE_B_PORT=39121
export P1_PERIPHERY_PORT=38120
export COMPOSE_FILE=ambient-compose.yaml
export COMPOSE_PROJECT_NAME=ambient-project
export COMPOSE_ENV_FILES=ambient.env
export COMPOSE_PROFILES=ambient-profile
export BUILDX_BUILDER=ambient-builder
run_wrapper config
expect_rc 0
if grep -Eq 'ambient-|example-only-not-for-runtime|P1_.*=' "$out"; then
  fail 'config exposed environment values'
fi
while IFS= read -r line; do
  case "$line" in
    Services:|Profiles:|core-a|core-b|mongo|periphery|toolbox|cross-core|tools|'') ;;
    *) fail "config printed non-allowlisted output: $line" ;;
  esac
done <"$out"
[ ! -s "$P1_TEST_ENV_LOG" ] ||
  fail "ambient Compose variable survived: $(cat "$P1_TEST_ENV_LOG")"
for name in \
  Services: Profiles: core-a core-b mongo periphery toolbox cross-core tools
do
  grep -Fqx "$name" "$out" || fail "config omitted $name"
done
assert_compose_prefixes
unset P1_MONGO_ROOT_USERNAME P1_MONGO_ROOT_PASSWORD P1_DATABASE_NAME
unset P1_INIT_ADMIN_USERNAME P1_INIT_ADMIN_PASSWORD P1_JWT_SECRET
unset P1_WEBHOOK_SECRET P1_FIRST_SERVER_NAME P1_DOCKER_SOCKET
unset P1_PERIPHERY_ROOT P1_MONGO_PORT P1_CORE_A_PORT P1_CORE_B_PORT
unset P1_PERIPHERY_PORT COMPOSE_FILE COMPOSE_PROJECT_NAME
unset COMPOSE_ENV_FILES COMPOSE_PROFILES BUILDX_BUILDER

reset_case
write_env
run_wrapper up
expect_rc 0
grep -Fq 'up -d --build mongo core-a' "$P1_TEST_DOCKER_LOG" ||
  fail 'up did not start Mongo and Core A first'
grep -Fq 'up -d --build --no-deps periphery' "$P1_TEST_DOCKER_LOG" ||
  fail 'up did not start Periphery after key export'
if grep -E 'up -d --build.*(core-b|toolbox)' "$P1_TEST_DOCKER_LOG"; then
  fail 'up selected an opt-in service'
fi
assert_compose_prefixes
assert_no_secrets_in_argv

reset_case
write_env
export P1_TEST_CORE_B_EXISTS=1
run_wrapper up
expect_failure
if grep -Fq 'up -d --build' "$P1_TEST_DOCKER_LOG"; then
  fail 'up mutated base while Core B existed'
fi
assert_compose_prefixes

reset_case
write_env
run_wrapper wait
expect_rc 0
grep -Fq -- '--profile tools run --rm --no-deps toolbox' \
  "$P1_TEST_DOCKER_LOG" || fail 'wait did not use isolated toolbox'
grep -Fq -- '--authenticationDatabase admin' "$P1_TEST_DOCKER_LOG" ||
  fail 'toolbox did not authenticate against admin'
grep -Fq 'P1_DATABASE_NAME' "$P1_TEST_DOCKER_LOG" ||
  fail 'toolbox did not select the lab database'
grep -Fq 'runCommand({ ping: 1 })' "$P1_TEST_DOCKER_LOG" ||
  fail 'toolbox did not perform an authenticated Mongo ping'
grep -Fq '/auth/login/LoginLocalUser' "$P1_TEST_CURL_LOG" ||
  fail 'wait did not authenticate to Core A'
grep -Fq '/read/GetSystemStats' "$P1_TEST_CURL_LOG" ||
  fail 'wait did not prove Periphery-backed stats'
grep -Fq '/read/ListDockerContainers' "$P1_TEST_CURL_LOG" ||
  fail 'wait did not prove selected-daemon container IDs'
assert_no_secrets_in_argv
assert_compose_prefixes

reset_case
write_env
export P1_TEST_VERSION_FAILURES=2
run_wrapper wait
expect_rc 0
[ "$(cat "$case_dir/version-count")" -ge 3 ] ||
  fail 'wait did not retry failed public readiness'

reset_case
write_env
export P1_TEST_INVALID_STATS=1
run_wrapper wait
expect_failure

reset_case
write_env
export P1_TEST_WRONG_DAEMON_IDS=1
run_wrapper wait
expect_failure

reset_case
write_env
run_wrapper cross-core-up
expect_rc 0
preflight_one=$(grep -n 'preflight:1' "$P1_TEST_TRACE" | cut -d: -f1)
pause_line=$(grep -n '^pause$' "$P1_TEST_TRACE" | cut -d: -f1)
preflight_two=$(grep -n 'preflight:2' "$P1_TEST_TRACE" | cut -d: -f1)
start_line=$(grep -n '^core-b-up$' "$P1_TEST_TRACE" | cut -d: -f1)
core_b_auth=$(grep -n 'curl:.*127.0.0.1:9121.*/auth/login' \
  "$P1_TEST_TRACE" | cut -d: -f1)
core_b_stats=$(grep -n 'curl:.*127.0.0.1:9121.*/read/GetSystemStats' \
  "$P1_TEST_TRACE" | cut -d: -f1)
unpause_line=$(grep -n '^unpause$' "$P1_TEST_TRACE" | tail -1 | cut -d: -f1)
[ "$preflight_one" -lt "$pause_line" ] &&
  [ "$pause_line" -lt "$preflight_two" ] &&
  [ "$preflight_two" -lt "$start_line" ] &&
  [ "$start_line" -lt "$core_b_auth" ] &&
  [ "$core_b_auth" -lt "$core_b_stats" ] &&
  [ "$core_b_stats" -lt "$unpause_line" ] ||
  fail 'cross-Core ordering contract violated'
for predicate in \
  in_progress_updates procedure_schedules action_schedules startup_actions \
  gitops_stacks gitops_resource_syncs run_at_startup \
  auto_deploy_git_updates auto_apply_updates files_on_host linked_repo commit
do
  grep -Fq "$predicate" "$P1_TEST_DOCKER_LOG" ||
    fail "cross-Core preflight omitted $predicate"
done
assert_no_secrets_in_argv
assert_compose_prefixes

for blocker in \
  in_progress_updates procedure_schedules action_schedules \
  startup_actions gitops_stacks gitops_resource_syncs
do
  reset_case
  write_env
  export P1_TEST_BLOCKER=$blocker
  run_wrapper cross-core-up
  expect_failure
  grep -Fq "$blocker" "$out" ||
    fail "unsafe preflight did not name $blocker"
  if grep -Eq '^pause$|^core-b-up$' "$P1_TEST_TRACE"; then
    fail "unsafe $blocker reached Core B mutation"
  fi
  assert_compose_prefixes
done

reset_case
write_env
export P1_TEST_SECOND_PREFLIGHT_FAIL=1
run_wrapper cross-core-up
expect_failure
grep -Fq 'unpause' "$P1_TEST_TRACE" ||
  fail 'second-preflight failure left Core A paused'
assert_compose_prefixes

reset_case
write_env
export P1_TEST_CORE_B_START_FAIL=1
run_wrapper cross-core-up
expect_failure
grep -Fq 'unpause' "$P1_TEST_TRACE" ||
  fail 'Core B start failure left Core A paused'
grep -Fq 'core-b-rm' "$P1_TEST_TRACE" ||
  fail 'Core B start failure did not remove attempted Core B'
assert_compose_prefixes

reset_case
write_env
export P1_TEST_CORE_B_READY_FAIL=1
run_wrapper cross-core-up
expect_failure
grep -Fq 'unpause' "$P1_TEST_TRACE" ||
  fail 'Core B readiness failure left Core A paused'
grep -Fq 'core-b-rm' "$P1_TEST_TRACE" ||
  fail 'Core B readiness failure did not remove attempted Core B'
assert_compose_prefixes

reset_case
write_env
export P1_TEST_CORE_B_EXISTS=1
run_wrapper cross-core-up
expect_failure
if grep -Fq 'preflight:' "$P1_TEST_TRACE"; then
  fail 'repeated cross-core-up reached preflight'
fi
assert_compose_prefixes

reset_case
export P1_TEST_BUILDER_ENDPOINT=ssh://remote-builder
run_wrapper build
expect_failure
[ ! -e "$P1_STATE_DIR" ] ||
  fail 'builder mismatch created runtime state'
if grep -Eq ' compose .* build core-a periphery' \
  "$P1_TEST_DOCKER_LOG"
then
  fail 'builder mismatch reached Compose build'
fi

reset_case
write_env
run_wrapper down
expect_rc 0
grep -Fq 'down --remove-orphans' "$P1_TEST_DOCKER_LOG" ||
  fail 'down omitted orphan cleanup'
grep -Fq -- '--profile cross-core --profile tools' \
  "$P1_TEST_DOCKER_LOG" || fail 'down omitted opt-in profiles'
if grep -Fq -- '--volumes' "$P1_TEST_DOCKER_LOG"; then
  fail 'ordinary down removed volumes'
fi
assert_compose_prefixes

reset_case
write_env
run_wrapper reset --yes
expect_rc 0
grep -Fq 'down --volumes --remove-orphans' "$P1_TEST_DOCKER_LOG" ||
  fail 'reset did not remove lab volumes and orphans'
grep -Fq -- '--profile cross-core --profile tools' \
  "$P1_TEST_DOCKER_LOG" || fail 'reset omitted opt-in profiles'
assert_compose_prefixes

reset_case
write_env
run_wrapper cross-core-down
expect_rc 0
grep -Fq 'rm --stop --force core-b' "$P1_TEST_DOCKER_LOG" ||
  fail 'cross-core-down did not remove Core B'
if grep -Eq 'rm --stop --force .*(mongo|core-a|periphery)' \
  "$P1_TEST_DOCKER_LOG"
then
  fail 'cross-core-down touched a base service'
fi
assert_compose_prefixes

printf 'P1 local wrapper tests OK\n'
