#!/usr/bin/env sh
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
wrapper=$repo/scripts/performance/p1-local.sh
build_context_check=$repo/scripts/performance/check-p1-local-build-context.sh
project=komodo-p1-local
compose_file=$repo/p1.local.compose.yaml
mongo_digest=sha256:ffa440e8d62533e24a67696ae1bbb46e610ebb3167d65abd122b496ae06d28e6
mongo_image=mongo:8.0.26@$mongo_digest
context_name=${P1_DOCKER_CONTEXT:-$(docker context show)}
state_dir=${P1_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/komodo-p1-local}
env_file=${P1_ENV_FILE:-$state_dir/p1-local.env}
artifact=${P1_RUNTIME_ARTIFACT:-$repo/target/p1-local-lab/runtime-proof.json}

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

canonical_file() {
  python3 - "$1" <<'PY'
import os
import sys

path = os.path.expanduser(sys.argv[1])
print(os.path.join(os.path.realpath(os.path.dirname(path)), os.path.basename(path)))
PY
}

state_dir=$(python3 - "$state_dir" <<'PY'
import os
import sys
print(os.path.realpath(os.path.expanduser(sys.argv[1])))
PY
)
env_file=$(canonical_file "$env_file")
artifact=$(canonical_file "$artifact")
artifact_tmp=$artifact.tmp

[ ! -L "$artifact" ] && [ ! -L "$artifact_tmp" ] ||
  fail 'runtime artifact destination must not be a symlink'
if git -C "$repo" ls-files --error-unmatch "$artifact" >/dev/null 2>&1; then
  fail 'runtime artifact destination must not be tracked'
fi
case "$artifact" in
  "$repo"/target/p1-local-lab/*)
    git -C "$repo" check-ignore -q "$artifact" ||
      fail 'runtime artifact destination must be ignored'
    ;;
  "$repo"/*) fail 'runtime artifact inside checkout must stay under target/p1-local-lab' ;;
  /*) ;;
  *) fail 'external runtime artifact path must be absolute' ;;
esac

mkdir -p "$(dirname -- "$artifact")"
rm -f "$artifact" "$artifact_tmp"

tmp=$(mktemp -d)
run_log=$tmp/runtime.log
cleanup_resources=$tmp/resources
: >"$run_log"
: >"$cleanup_resources"
run_id=$(date -u +%Y%m%dT%H%M%SZ)-$$
prefix=p1-runtime-$run_id
failure_dir=$repo/target/p1-local-lab/failure-logs
core_a_paused=0
periphery_stopped=0
core_b_attempted=0
outside_canary=
jwt=

docker_cmd() {
  docker --context "$context_name" "$@"
}

compose_cmd() {
  docker_cmd compose \
    --project-name "$project" \
    --env-file "$env_file" \
    --file "$compose_file" "$@"
}

project_ids() {
  kind=$1
  case "$kind" in
    container) docker_cmd ps --all --quiet --filter "label=com.docker.compose.project=$project" ;;
    network) docker_cmd network ls --quiet --filter "label=com.docker.compose.project=$project" ;;
    volume) docker_cmd volume ls --quiet --filter "label=com.docker.compose.project=$project" ;;
  esac
}

deadline_run_log() {
  seconds=$1
  log_path=$2
  shift 2
  python3 - "$seconds" "$log_path" "$@" <<'PY'
import os
import signal
import subprocess
import sys

timeout = int(sys.argv[1])
log_path = sys.argv[2]
command = sys.argv[3:]
with open(log_path, "ab") as log:
    process = subprocess.Popen(
        command,
        stdout=log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    try:
        return_code = process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        raise SystemExit(124)
raise SystemExit(return_code)
PY
}

deadline_run() {
  seconds=$1
  shift
  deadline_run_log "$seconds" "$run_log" "$@"
}

run_wrapper() {
  seconds=$1
  shift
  deadline_run "$seconds" env \
    P1_DOCKER_CONTEXT="$context_name" \
    P1_ENV_FILE="$env_file" \
    "$wrapper" "$@"
}

expect_cross_refusal() {
  expected=$1
  case_log=$tmp/refusal-$expected.log
  set +e
  deadline_run_log 270 "$case_log" env \
    P1_DOCKER_CONTEXT="$context_name" \
    P1_ENV_FILE="$env_file" \
    "$wrapper" cross-core-up
  refusal_rc=$?
  set -e
  cat "$case_log" >>"$run_log"
  [ "$refusal_rc" -ne 0 ] || fail "Core B accepted blocker: $expected"
  grep -Fq "cross-Core preflight refused: $expected" "$case_log" ||
    fail "Core B refusal did not prove blocker: $expected"
  [ -z "$(compose_cmd --profile cross-core ps -q core-b)" ] ||
    fail "Core B exists after refusal: $expected"
}

read_env_value() {
  key=$1
  awk -F= -v key="$key" '$1 == key { sub(/^[^=]*=/, ""); print; exit }' \
    "$env_file"
}

redact_file() {
  source=$1
  destination=$2
  python3 - "$env_file" "$source" "$destination" <<'PY'
import sys

env_path, source_path, destination_path = sys.argv[1:]
secret_keys = {
    "P1_MONGO_ROOT_PASSWORD",
    "P1_INIT_ADMIN_PASSWORD",
    "P1_JWT_SECRET",
    "P1_WEBHOOK_SECRET",
}
secrets = []
with open(env_path, encoding="utf-8") as env:
    for line in env:
        key, separator, value = line.rstrip("\n").partition("=")
        if separator and key in secret_keys and value:
            secrets.append(value)
with open(source_path, encoding="utf-8", errors="replace") as source:
    output = source.read()
for secret in secrets:
    output = output.replace(secret, "<redacted>")
with open(destination_path, "w", encoding="utf-8") as destination:
    destination.write(output)
PY
}

mongo_eval() {
  javascript=$1
  compose_cmd --profile tools run --rm --no-deps toolbox \
    sh -ec 'exec mongosh --quiet \
      --host mongo --port 27017 \
      --username "$MONGO_INITDB_ROOT_USERNAME" \
      --password "$MONGO_INITDB_ROOT_PASSWORD" \
      --authenticationDatabase admin --eval "$1"' sh "$javascript"
}

login_core() {
  port=$1
  username=$(read_env_value P1_INIT_ADMIN_USERNAME)
  password=$(read_env_value P1_INIT_ADMIN_PASSWORD)
  response=$tmp/login-$port.json
  if ! printf '{"username":"%s","password":"%s"}' "$username" "$password" |
    curl --silent --show-error --fail --connect-timeout 2 --max-time 10 \
      --header 'Content-Type: application/json' --data-binary @- \
      "http://127.0.0.1:$port/auth/login/LoginLocalUser" >"$response"
  then
    fail "Core login failed on port $port"
  fi
  jwt=$(jq -er 'select(.type == "Jwt") | .data.jwt' "$response") ||
    fail "Core login did not return Jwt on port $port"
}

api_request() {
  port=$1
  route=$2
  body=$3
  output=$4
  {
    printf 'header = "Authorization: Bearer %s"\n' "$jwt"
    printf '%s\n' 'header = "Content-Type: application/json"'
    printf 'data = "%s"\n' "$(printf '%s' "$body" | sed 's/"/\\"/g')"
  } | curl --silent --show-error --fail --connect-timeout 2 --max-time 15 \
    --config - "http://127.0.0.1:$port$route" >"$output"
}

create_resource() {
  kind=$1
  name=$2
  config=$3
  response=$tmp/create-$kind-$$.json
  body=$(jq -cn --arg name "$name" --argjson config "$config" \
    '{name: $name, config: $config}')
  api_request 9120 "/write/Create$kind" "$body" "$response"
  id=$(jq -er '.id // ._id["$oid"] // ._id' "$response")
  printf '%s|%s\n' "$kind" "$id" >>"$cleanup_resources"
  printf '%s\n' "$id"
}

delete_resource() {
  kind=$1
  id=$2
  response=$tmp/delete-$kind-$$.json
  body=$(jq -cn --arg id "$id" '{id: $id}')
  api_request 9120 "/write/Delete$kind" "$body" "$response" || true
  marker=$kind'|'$id
  grep -Fvx "$marker" "$cleanup_resources" >"$cleanup_resources.next" || true
  mv "$cleanup_resources.next" "$cleanup_resources"
}

cleanup_markers() {
  [ -f "$env_file" ] || return 0
  [ -n "$(project_ids container)" ] || return 0
  mongo_eval "const lab=db.getSiblingDB(process.env.P1_DATABASE_NAME);
    for (const name of ['Update','Procedure','Action','Stack','ResourceSync','P1HarnessProof']) {
      lab.getCollection(name).deleteMany({\$or:[{_p1_local_harness:'$run_id'},{'other_data._p1_local_harness':'$run_id'},{name:{\$regex:'^$prefix'}}]});
    }" >/dev/null 2>&1 || true
}

cleanup_api_resources() {
  [ -n "$jwt" ] || return 0
  [ -f "$cleanup_resources" ] || return 0
  while IFS='|' read -r kind id; do
    [ -n "$kind" ] || continue
    response=$tmp/cleanup-$kind-$$.json
    body=$(jq -cn --arg id "$id" '{id: $id}')
    api_request 9120 "/write/Delete$kind" "$body" "$response" >/dev/null 2>&1 || true
  done <"$cleanup_resources"
  : >"$cleanup_resources"
}

save_failure_logs() {
  [ -f "$env_file" ] || return 0
  mkdir -p "$failure_dir"
  raw=$tmp/compose-failure.log
  cp "$run_log" "$raw"
  compose_cmd logs --tail 200 mongo core-a periphery core-b >>"$raw" 2>&1 || true
  redact_file "$raw" "$failure_dir/$run_id.log" || true
}

cleanup() {
  cleanup_rc=$?
  set +e
  if [ "$core_a_paused" -eq 1 ]; then
    compose_cmd unpause core-a >/dev/null 2>&1
  fi
  if [ "$periphery_stopped" -eq 1 ]; then
    compose_cmd start periphery >/dev/null 2>&1
  fi
  if [ "$core_b_attempted" -eq 1 ]; then
    compose_cmd --profile cross-core rm --stop --force core-b >/dev/null 2>&1
  fi
  cleanup_api_resources
  cleanup_markers
  if [ -n "$outside_canary" ]; then
    docker_cmd volume rm --force "$outside_canary" >/dev/null 2>&1
  fi
  if [ "$cleanup_rc" -ne 0 ]; then
    save_failure_logs
  fi
  rm -rf "$tmp"
  exit "$cleanup_rc"
}
trap cleanup EXIT HUP INT TERM

for kind in container network volume; do
  [ -z "$(project_ids "$kind")" ] || {
    printf '%s\n' 'Existing komodo-p1-local resources found.' >&2
    printf '%s\n' 'Run: scripts/performance/p1-local.sh reset --yes' >&2
    fail 'runtime verifier requires an empty lab project'
  }
done

endpoint=$(docker context inspect "$context_name" \
  --format '{{ (index .Endpoints "docker").Host }}')
case "$endpoint" in unix:///*) ;; *) fail 'runtime verifier requires a local unix Docker context' ;; esac

git_sha=$(git -C "$repo" rev-parse HEAD)
if [ -n "$(git -C "$repo" status --porcelain --untracked-files=all)" ]; then
  git_dirty=true
else
  git_dirty=false
fi
host_arch=$(uname -m)
engine_arch=$(docker_cmd info --format '{{.Architecture}}')

printf '%s\n' 'P1 runtime: doctor'
run_wrapper 300 doctor || fail 'doctor failed'
printf '%s\n' 'P1 runtime: BuildKit context isolation'
deadline_run 300 env P1_DOCKER_CONTEXT="$context_name" "$build_context_check" ||
  fail 'BuildKit context isolation failed'
printf '%s\n' 'P1 runtime: source build'
run_wrapper 7200 build || fail 'source build failed'
printf '%s\n' 'P1 runtime: base stack'
run_wrapper 7200 up || fail 'base stack startup failed'
run_wrapper 150 wait || fail 'base readiness failed'

login_core 9120

[ -z "$(compose_cmd --profile cross-core ps -q core-b)" ] ||
  fail 'Core B started by default'
cross_absent=true

core_version=$(curl --silent --show-error --fail http://127.0.0.1:9120/version)
periphery_version=$(curl --silent --show-error --fail http://127.0.0.1:8120/version)
ui_status=$(curl --silent --output /dev/null --write-out '%{http_code}' \
  http://127.0.0.1:9120/)
[ "$ui_status" = 200 ] || fail 'embedded UI did not return HTTP 200'

mongo_id=$(docker_cmd image inspect "$mongo_image" --format '{{.Id}}')
core_image_id=$(docker_cmd image inspect komodo-p1-local-core:dev --format '{{.Id}}')
periphery_image_id=$(docker_cmd image inspect komodo-p1-local-periphery:dev --format '{{.Id}}')

inspection=$tmp/inspection.json
for service in mongo core-a periphery; do
  id=$(compose_cmd ps -q "$service")
  [ -n "$id" ] || fail "missing service: $service"
  docker_cmd inspect "$id"
done | jq -s 'add' >"$inspection"

jq -e 'all(.[]; ([.NetworkSettings.Ports[]?[]?.HostIp] | all(. == "127.0.0.1")))' \
  "$inspection" >/dev/null || fail 'a service port is not loopback-only'
all_ports_loopback=true
jq -e '([.[] | select(any(.Mounts[]?; .Destination == "/var/run/docker.sock")) | .Config.Labels["com.docker.compose.service"]] == ["periphery"])' \
  "$inspection" >/dev/null || fail 'Docker socket is not restricted to Periphery'
docker_socket_only=true

docker_list_response=$tmp/docker-containers.json
api_request 9120 /read/ListDockerContainers \
  '{"server":"p1-local"}' "$docker_list_response"
mongo_container=$(compose_cmd ps -q mongo)
core_a_container=$(compose_cmd ps -q core-a)
periphery_container=$(compose_cmd ps -q periphery)
jq -e \
  --arg mongo "$mongo_container" \
  --arg core_a "$core_a_container" \
  --arg periphery "$periphery_container" '
    [.[].id] as $ids |
    ($ids | index($mongo)) != null and
    ($ids | index($core_a)) != null and
    ($ids | index($periphery)) != null
  ' "$docker_list_response" >/dev/null ||
  fail 'Periphery Docker API does not report the selected daemon containers'
periphery_docker_same=true

case "$state_dir" in "$repo"|"$repo"/*) fail 'runtime state is inside checkout' ;; esac
runtime_state_outside=true
[ -z "$(compose_cmd --profile tools ps -q toolbox)" ] ||
  fail 'toolbox is unexpectedly long-running'

printf '%s\n' 'P1 runtime: bounded wait failure and recovery'
compose_cmd stop periphery >/dev/null
periphery_stopped=1
if run_wrapper 150 wait; then
  fail 'wait succeeded while Periphery was stopped'
fi
compose_cmd start periphery >/dev/null
periphery_stopped=0
run_wrapper 150 wait || fail 'wait did not recover after Periphery restart'

procedure_id=$(create_resource Procedure "$prefix-procedure" '{"stages":[]}')
run_response=$tmp/run-procedure.json
api_request 9120 /execute/RunProcedure \
  "$(jq -cn --arg procedure "$procedure_id" '{procedure:$procedure}')" \
  "$run_response"
update_id=$(jq -er '.id // ._id["$oid"] // ._id' "$run_response")
complete=false
attempt=0
while [ "$attempt" -lt 60 ]; do
  update_response=$tmp/get-update.json
  api_request 9120 /read/GetUpdate \
    "$(jq -cn --arg id "$update_id" '{id:$id}')" "$update_response"
  status=$(jq -r '.status' "$update_response")
  if [ "$status" = Complete ]; then complete=true; break; fi
  [ "$status" != Failed ] || fail 'no-op Procedure failed'
  attempt=$((attempt + 1))
  sleep 1
done
[ "$complete" = true ] || fail 'no-op Procedure did not complete'

mongo_eval "const lab=db.getSiblingDB(process.env.P1_DATABASE_NAME);
  const source=lab.Update.findOne({\$or:[{_id:ObjectId('$update_id')},{_id:'$update_id'}]});
  if(!source) quit(2);
  source._id=new ObjectId(); source.status='InProgress'; source.end_ts=null;
  source.start_ts=Date.now();
  source.other_data={...source.other_data,_p1_local_harness:'$run_id'};
  source._p1_local_harness='$run_id'; lab.Update.insertOne(source);" >/dev/null
expect_cross_refusal in_progress_updates
in_progress_refused=true
mongo_eval "db.getSiblingDB(process.env.P1_DATABASE_NAME).Update.deleteMany({_p1_local_harness:'$run_id'});" >/dev/null
delete_resource Procedure "$procedure_id"

procedure_id=$(create_resource Procedure "$prefix-procedure-schedule-base" '{"stages":[]}')
mongo_eval "const lab=db.getSiblingDB(process.env.P1_DATABASE_NAME); const source=lab.Procedure.findOne({name:'$prefix-procedure-schedule-base'}); if(!source) quit(2); source._id=new ObjectId(); source.name='$prefix-procedure-schedule'; source.config.schedule='0 0 0 1 1 ?'; source.config.schedule_enabled=true; source._p1_local_harness='$run_id'; lab.Procedure.insertOne(source);" >/dev/null
expect_cross_refusal procedure_schedules
procedure_schedule_refused=true
mongo_eval "db.getSiblingDB(process.env.P1_DATABASE_NAME).Procedure.deleteMany({_p1_local_harness:'$run_id'});" >/dev/null
delete_resource Procedure "$procedure_id"

action_id=$(create_resource Action "$prefix-action-schedule-base" '{"file_contents":""}')
mongo_eval "const lab=db.getSiblingDB(process.env.P1_DATABASE_NAME); const source=lab.Action.findOne({name:'$prefix-action-schedule-base'}); if(!source) quit(2); source._id=new ObjectId(); source.name='$prefix-action-schedule'; source.config.schedule='0 0 0 1 1 ?'; source.config.schedule_enabled=true; source._p1_local_harness='$run_id'; lab.Action.insertOne(source);" >/dev/null
expect_cross_refusal action_schedules
action_schedule_refused=true
mongo_eval "db.getSiblingDB(process.env.P1_DATABASE_NAME).Action.deleteMany({_p1_local_harness:'$run_id'});" >/dev/null
delete_resource Action "$action_id"

action_id=$(create_resource Action "$prefix-action-startup-base" '{"file_contents":""}')
mongo_eval "const lab=db.getSiblingDB(process.env.P1_DATABASE_NAME); const source=lab.Action.findOne({name:'$prefix-action-startup-base'}); if(!source) quit(2); source._id=new ObjectId(); source.name='$prefix-action-startup'; source.config.run_at_startup=true; source._p1_local_harness='$run_id'; lab.Action.insertOne(source);" >/dev/null
expect_cross_refusal startup_actions
startup_refused=true
mongo_eval "db.getSiblingDB(process.env.P1_DATABASE_NAME).Action.deleteMany({_p1_local_harness:'$run_id'});" >/dev/null
delete_resource Action "$action_id"

printf '%s\n' 'P1 runtime: Core B preflight refuses GitOps Stack'
stack_id=$(create_resource Stack "$prefix-stack-base" '{}') ||
  fail 'failed to create baseline Stack'
mongo_eval "const lab=db.getSiblingDB(process.env.P1_DATABASE_NAME); const source=lab.Stack.findOne({_id:ObjectId('$stack_id')}); if(!source) quit(2); source._id=new ObjectId(); source.name='$prefix-stack'; source.config.repo='p1-local/no-fetch'; source.config.commit=''; source.config.files_on_host=false; source.config.auto_deploy_git_updates=true; source._p1_local_harness='$run_id'; lab.Stack.insertOne(source);" >/dev/null ||
  fail 'failed to create GitOps Stack blocker fixture'
expect_cross_refusal gitops_stacks
gitops_stack_refused=true
mongo_eval "db.getSiblingDB(process.env.P1_DATABASE_NAME).Stack.deleteMany({_p1_local_harness:'$run_id'});" >/dev/null
delete_resource Stack "$stack_id"

printf '%s\n' 'P1 runtime: Core B preflight refuses GitOps Resource Sync'
sync_id=$(create_resource ResourceSync "$prefix-sync-base" '{}') ||
  fail 'failed to create baseline Resource Sync'
mongo_eval "const lab=db.getSiblingDB(process.env.P1_DATABASE_NAME); const source=lab.ResourceSync.findOne({_id:ObjectId('$sync_id')}); if(!source) quit(2); source._id=new ObjectId(); source.name='$prefix-sync'; source.config.repo='p1-local/no-fetch'; source.config.commit=''; source.config.files_on_host=false; source.config.auto_apply_updates=true; source._p1_local_harness='$run_id'; lab.ResourceSync.insertOne(source);" >/dev/null ||
  fail 'failed to create GitOps Resource Sync blocker fixture'
expect_cross_refusal gitops_resource_syncs
gitops_sync_refused=true
mongo_eval "db.getSiblingDB(process.env.P1_DATABASE_NAME).ResourceSync.deleteMany({_p1_local_harness:'$run_id'});" >/dev/null
delete_resource ResourceSync "$sync_id"

printf '%s\n' 'P1 runtime: safe cross-Core success'
core_b_attempted=1
run_wrapper 270 cross-core-up || fail 'safe Core B startup failed'
core_b_container=$(compose_cmd --profile cross-core ps -q core-b)
[ -n "$core_b_container" ] || fail 'Core B is absent after successful startup'
core_b_image_id=$(docker_cmd inspect "$core_b_container" --format '{{.Image}}')
core_a_container=$(compose_cmd ps -q core-a)
core_a_container_image=$(docker_cmd inspect "$core_a_container" --format '{{.Image}}')
[ "$core_b_image_id" = "$core_a_container_image" ] || fail 'Core A and Core B images differ'
core_b_version=$(curl --silent --show-error --fail http://127.0.0.1:9121/version)
login_core 9121
stats_response=$tmp/core-b-stats.json
api_request 9121 /read/GetSystemStats '{"server":"p1-local"}' "$stats_response"
jq -e '.cpu_perc | type == "number"' "$stats_response" >/dev/null ||
  fail 'Core B authenticated stats failed'
cross_stats=true
[ "$(docker_cmd inspect "$core_a_container" --format '{{.State.Paused}}')" = false ] ||
  fail 'Core A remained paused after Core B startup'
core_a_unpaused=true
run_wrapper 300 cross-core-down || fail 'cross-core-down failed'
core_b_attempted=0
[ -z "$(compose_cmd --profile cross-core ps -q core-b)" ] || fail 'Core B was not removed independently'
run_wrapper 150 wait ||
  fail 'base readiness did not recover after cross-core-down'
for service in mongo core-a periphery; do
  id=$(compose_cmd ps -q "$service")
  [ "$(docker_cmd inspect "$id" --format '{{.State.Health.Status}}')" = healthy ] ||
    fail "$service is unhealthy after cross-core-down"
done
cross_removed=true

printf '%s\n' 'P1 runtime: persistence and reset isolation'
mongo_eval "db.getSiblingDB(process.env.P1_DATABASE_NAME).P1HarnessProof.insertOne({_p1_local_harness:'$run_id',value:'persist'});" >/dev/null
run_wrapper 300 down || fail 'ordinary down failed'
run_wrapper 7200 up || fail 'base restart failed'
sentinel=$(mongo_eval "print(db.getSiblingDB(process.env.P1_DATABASE_NAME).P1HarnessProof.countDocuments({_p1_local_harness:'$run_id'}));")
[ "$sentinel" = 1 ] || fail 'ordinary down did not preserve Mongo sentinel'
down_preserved=true

outside_canary=p1-local-outside-$run_id
docker_cmd volume create "$outside_canary" >/dev/null
if run_wrapper 300 reset; then fail 'reset without --yes succeeded'; fi
[ -n "$(project_ids container)" ] || fail 'reset without --yes changed the lab'
reset_refused=true
run_wrapper 300 reset --yes || fail 'reset --yes failed'
for kind in container network volume; do
  [ -z "$(project_ids "$kind")" ] || fail "reset left project $kind resources"
done
reset_removed=true
docker_cmd volume inspect "$outside_canary" >/dev/null || fail 'reset removed outside canary'
outside_preserved=true
[ -f "$env_file" ] || fail 'reset removed external runtime environment'
docker_cmd volume rm "$outside_canary" >/dev/null
outside_canary=

captured_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
candidate=$tmp/runtime-proof.json
jq -n \
  --arg git_sha "$git_sha" \
  --argjson git_dirty "$git_dirty" \
  --arg captured "$captured_at" \
  --arg context "$context_name" \
  --arg host_arch "$host_arch" \
  --arg engine_arch "$engine_arch" \
  --arg mongo_id "$mongo_id" \
  --arg mongo_digest "$mongo_digest" \
  --arg core_id "$core_image_id" \
  --arg core_b_id "$core_b_image_id" \
  --arg periphery_id "$periphery_image_id" \
  --arg core_version "$core_version" \
  --arg core_b_version "$core_b_version" \
  --arg periphery_version "$periphery_version" '
  {
    git_sha: $git_sha,
    git_dirty: $git_dirty,
    captured_at_utc: $captured,
    compose_project: "komodo-p1-local",
    docker_context: $context,
    docker_endpoint_kind: "unix",
    host_architecture: $host_arch,
    engine_architecture: $engine_arch,
    images: {
      mongo: {id: $mongo_id, version: "8.0.26", digest: $mongo_digest},
      core_a: {id: $core_id, version: $core_version},
      core_b: {id: $core_b_id, version: $core_b_version},
      periphery: {id: $periphery_id, version: $periphery_version}
    },
    readiness: {
      core_version: $core_version,
      periphery_version: $periphery_version,
      compose_service_health: {
        mongo: "healthy", core_a: "healthy", periphery: "healthy"
      },
      authenticated_system_stats: true,
      embedded_ui_http_status: 200
    },
    security: {
      all_ports_loopback_only: true,
      docker_socket_only_in_periphery: true,
      periphery_docker_api_same_daemon: true,
      runtime_state_outside_checkout: true,
      secret_output_scan_passed: true
    },
    cross_core: {
      absent_by_default: true,
      in_progress_update_refused: true,
      procedure_schedule_refused: true,
      action_schedule_refused: true,
      run_at_startup_refused: true,
      gitops_stack_refused: true,
      gitops_resource_sync_refused: true,
      authenticated_system_stats: true,
      core_a_unpaused: true,
      removed_independently: true
    },
    lifecycle: {
      down_preserved_mongo_sentinel: true,
      reset_without_yes_refused: true,
      reset_removed_project_resources: true,
      outside_canary_preserved: true
    }
  }' >"$candidate"

for key in P1_MONGO_ROOT_PASSWORD P1_INIT_ADMIN_PASSWORD P1_JWT_SECRET P1_WEBHOOK_SECRET; do
  secret=$(read_env_value "$key")
  [ -n "$secret" ] || fail "missing secret for output scan: $key"
  if grep -Fq "$secret" "$run_log" "$candidate"; then
    fail "secret value leaked into runtime output: $key"
  fi
done

jq -e '
  ([.. | objects | keys[]] | any(
    . == "password" or . == "username" or . == "jwt" or
    . == "credentials" or . == "mongo_uri" or
    . == "database_uri" or . == "environment" or
    . == "env_file" or . == "stdout" or . == "stderr" or
    . == "command_log"
  ) | not) and
  ([.. | strings] | any(test("^mongodb(\\+srv)?://")) | not) and
  .readiness.authenticated_system_stats == true and
  .security.periphery_docker_api_same_daemon == true and
  .cross_core.in_progress_update_refused == true and
  .cross_core.procedure_schedule_refused == true and
  .cross_core.action_schedule_refused == true and
  .cross_core.run_at_startup_refused == true and
  .cross_core.gitops_stack_refused == true and
  .cross_core.gitops_resource_sync_refused == true and
  .lifecycle.down_preserved_mongo_sentinel == true and
  .lifecycle.reset_removed_project_resources == true and
  .lifecycle.outside_canary_preserved == true
' "$candidate" >/dev/null || fail 'runtime proof candidate is invalid'

cp "$candidate" "$artifact_tmp"
chmod 0600 "$artifact_tmp"
mv "$artifact_tmp" "$artifact"

trap - EXIT HUP INT TERM
rm -rf "$tmp"
printf 'P1 local runtime proof OK: %s\n' "$artifact"
