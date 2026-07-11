#!/usr/bin/env sh
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
project=komodo-p1-local
compose_file=$repo/p1.local.compose.yaml
example_env=$repo/compose/p1.local.env.example
mongo_image='mongo:8.0.26@sha256:ffa440e8d62533e24a67696ae1bbb46e610ebb3167d65abd122b496ae06d28e6'

usage() {
  printf '%s\n' \
    'usage: p1-local.sh {doctor|config|build|up|wait|cross-core-up|cross-core-down|status|down|reset --yes}' >&2
  exit 64
}

die() {
  printf 'ERROR: %s\n' "$1" >&2
  exit 1
}

command=${1-}
case "$command" in
  reset)
    [ "$#" -eq 2 ] && [ "${2-}" = --yes ] || {
      printf '%s\n' 'reset requires --yes' >&2
      exit 64
    }
    ;;
  doctor|config|build|up|wait|cross-core-up|cross-core-down|status|down)
    [ "$#" -eq 1 ] || usage
    ;;
  *) usage ;;
esac

host_state=${P1_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/komodo-p1-local}
host_env=${P1_ENV_FILE-}
host_context=${P1_DOCKER_CONTEXT-}
host_socket=${P1_DOCKER_SOCKET-}
host_root=${P1_PERIPHERY_ROOT-}
host_mongo_port=${P1_MONGO_PORT-}
host_core_a_port=${P1_CORE_A_PORT-}
host_core_b_port=${P1_CORE_B_PORT-}
host_periphery_port=${P1_PERIPHERY_PORT-}

unset P1_STATE_DIR P1_ENV_FILE P1_DOCKER_CONTEXT
unset P1_MONGO_ROOT_USERNAME P1_MONGO_ROOT_PASSWORD P1_DATABASE_NAME
unset P1_INIT_ADMIN_USERNAME P1_INIT_ADMIN_PASSWORD P1_JWT_SECRET
unset P1_WEBHOOK_SECRET P1_FIRST_SERVER_NAME P1_DOCKER_SOCKET
unset P1_PERIPHERY_ROOT P1_MONGO_PORT P1_CORE_A_PORT P1_CORE_B_PORT
unset P1_PERIPHERY_PORT COMPOSE_FILE COMPOSE_PROJECT_NAME
unset COMPOSE_ENV_FILES COMPOSE_PROFILES BUILDX_BUILDER

canonical_path() {
  python3 - "$1" <<'PY'
import os
import sys
print(os.path.realpath(os.path.expanduser(sys.argv[1])))
PY
}

canonical_file() {
  python3 - "$1" <<'PY'
import os
import sys
p = os.path.expanduser(sys.argv[1])
print(os.path.join(os.path.realpath(os.path.dirname(p)), os.path.basename(p)))
PY
}

state_dir=$(canonical_path "$host_state")
if [ -n "$host_env" ]; then
  env_file=$(canonical_file "$host_env")
else
  env_file=$state_dir/p1-local.env
fi

path_outside_repo() {
  case "$1" in
    "$repo"|"$repo"/*) die "$2 must be outside the checkout" ;;
  esac
}

path_outside_repo "$(canonical_path "$state_dir")" 'P1_STATE_DIR'
canonical_env_file=$(canonical_file "$env_file")
if [ "$canonical_env_file" != "$example_env" ]; then
  path_outside_repo "$canonical_env_file" 'P1_ENV_FILE'
fi

selected_context=$host_context
docker_endpoint=

prepare_context() {
  if [ -z "$selected_context" ]; then
    selected_context=$(docker context show) ||
      die 'unable to resolve Docker context'
  fi
  docker_endpoint=$(docker context inspect "$selected_context" \
    --format '{{ (index .Endpoints "docker").Host }}') ||
    die "Docker context does not exist: $selected_context"
  case "$docker_endpoint" in
    unix:///*) ;;
    *) die 'Docker context must use a local unix endpoint' ;;
  esac
}

docker_cmd() {
  docker --context "$selected_context" "$@"
}

compose_cmd() {
  docker_cmd compose \
    --project-name "$project" \
    --env-file "$env_file" \
    --file "$compose_file" "$@"
}

validate_env_metadata() {
  python3 - "$env_file" <<'PY'
import os
import stat
import sys

path = sys.argv[1]
try:
    meta = os.lstat(path)
except FileNotFoundError:
    print("runtime environment does not exist", file=sys.stderr)
    raise SystemExit(1)
if stat.S_ISLNK(meta.st_mode) or not stat.S_ISREG(meta.st_mode):
    print("runtime environment must be a regular file", file=sys.stderr)
    raise SystemExit(1)
if meta.st_uid != os.getuid():
    print("runtime environment must be owned by the current user", file=sys.stderr)
    raise SystemExit(1)
if stat.S_IMODE(meta.st_mode) != 0o600:
    print("runtime environment must have mode 0600", file=sys.stderr)
    raise SystemExit(1)
PY
}

clear_loaded_env() {
  v_P1_MONGO_ROOT_USERNAME=
  v_P1_MONGO_ROOT_PASSWORD=
  v_P1_DATABASE_NAME=
  v_P1_INIT_ADMIN_USERNAME=
  v_P1_INIT_ADMIN_PASSWORD=
  v_P1_JWT_SECRET=
  v_P1_WEBHOOK_SECRET=
  v_P1_FIRST_SERVER_NAME=
  v_P1_DOCKER_SOCKET=
  v_P1_PERIPHERY_ROOT=
  v_P1_MONGO_PORT=
  v_P1_CORE_A_PORT=
  v_P1_CORE_B_PORT=
  v_P1_PERIPHERY_PORT=
}

load_runtime_env() {
  [ "$env_file" != "$example_env" ] ||
    die 'runtime commands refuse the example environment file'
  validate_env_metadata || exit 1
  clear_loaded_env
  seen='|'
  count=0
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      *=*) key=${line%%=*}; value=${line#*=} ;;
      *) die 'runtime environment contains an invalid line' ;;
    esac
    case "$key" in
      P1_MONGO_ROOT_USERNAME|P1_MONGO_ROOT_PASSWORD|P1_DATABASE_NAME|P1_INIT_ADMIN_USERNAME|P1_INIT_ADMIN_PASSWORD|P1_JWT_SECRET|P1_WEBHOOK_SECRET|P1_FIRST_SERVER_NAME|P1_DOCKER_SOCKET|P1_PERIPHERY_ROOT|P1_MONGO_PORT|P1_CORE_A_PORT|P1_CORE_B_PORT|P1_PERIPHERY_PORT) ;;
      *) die "runtime environment contains unknown key: $key" ;;
    esac
    case "$seen" in *"|$key|"*) die "runtime environment contains duplicate key: $key" ;; esac
    seen=$seen$key'|'
    count=$((count + 1))
    case "$key" in
      P1_MONGO_ROOT_USERNAME) v_P1_MONGO_ROOT_USERNAME=$value ;;
      P1_MONGO_ROOT_PASSWORD) v_P1_MONGO_ROOT_PASSWORD=$value ;;
      P1_DATABASE_NAME) v_P1_DATABASE_NAME=$value ;;
      P1_INIT_ADMIN_USERNAME) v_P1_INIT_ADMIN_USERNAME=$value ;;
      P1_INIT_ADMIN_PASSWORD) v_P1_INIT_ADMIN_PASSWORD=$value ;;
      P1_JWT_SECRET) v_P1_JWT_SECRET=$value ;;
      P1_WEBHOOK_SECRET) v_P1_WEBHOOK_SECRET=$value ;;
      P1_FIRST_SERVER_NAME) v_P1_FIRST_SERVER_NAME=$value ;;
      P1_DOCKER_SOCKET) v_P1_DOCKER_SOCKET=$value ;;
      P1_PERIPHERY_ROOT) v_P1_PERIPHERY_ROOT=$value ;;
      P1_MONGO_PORT) v_P1_MONGO_PORT=$value ;;
      P1_CORE_A_PORT) v_P1_CORE_A_PORT=$value ;;
      P1_CORE_B_PORT) v_P1_CORE_B_PORT=$value ;;
      P1_PERIPHERY_PORT) v_P1_PERIPHERY_PORT=$value ;;
    esac
  done <"$env_file"
  [ "$count" -eq 14 ] || die 'runtime environment must contain all 14 keys'
  for key in P1_MONGO_ROOT_USERNAME P1_MONGO_ROOT_PASSWORD P1_DATABASE_NAME P1_INIT_ADMIN_USERNAME P1_INIT_ADMIN_PASSWORD P1_JWT_SECRET P1_WEBHOOK_SECRET P1_FIRST_SERVER_NAME P1_DOCKER_SOCKET P1_PERIPHERY_ROOT P1_MONGO_PORT P1_CORE_A_PORT P1_CORE_B_PORT P1_PERIPHERY_PORT; do
    case "$seen" in *"|$key|"*) ;; *) die "runtime environment missing key: $key" ;; esac
  done
  for value in \
    "$v_P1_MONGO_ROOT_USERNAME" "$v_P1_MONGO_ROOT_PASSWORD" \
    "$v_P1_DATABASE_NAME" "$v_P1_INIT_ADMIN_USERNAME" \
    "$v_P1_INIT_ADMIN_PASSWORD" "$v_P1_JWT_SECRET" \
    "$v_P1_WEBHOOK_SECRET" "$v_P1_FIRST_SERVER_NAME" \
    "$v_P1_DOCKER_SOCKET" "$v_P1_PERIPHERY_ROOT" \
    "$v_P1_MONGO_PORT" "$v_P1_CORE_A_PORT" \
    "$v_P1_CORE_B_PORT" "$v_P1_PERIPHERY_PORT"
  do
    [ -n "$value" ] || die 'runtime environment values must be nonempty'
  done
  case "$v_P1_DOCKER_SOCKET" in /*) ;; *) die 'P1_DOCKER_SOCKET must be absolute' ;; esac
  case "$v_P1_PERIPHERY_ROOT" in /*) ;; *) die 'P1_PERIPHERY_ROOT must be absolute' ;; esac
  runtime_root=$(canonical_path "$v_P1_PERIPHERY_ROOT")
  path_outside_repo "$runtime_root" 'P1_PERIPHERY_ROOT'
  python3 - "$v_P1_MONGO_PORT" "$v_P1_CORE_A_PORT" \
    "$v_P1_CORE_B_PORT" "$v_P1_PERIPHERY_PORT" <<'PY'
import sys

try:
    ports = [int(value) for value in sys.argv[1:]]
except ValueError:
    raise SystemExit("runtime ports must be integers")
if len(set(ports)) != len(ports) or any(port < 1 or port > 65535 for port in ports):
    raise SystemExit("runtime ports must be unique integers from 1 through 65535")
PY
}

create_runtime_env() {
  [ ! -e "$env_file" ] && [ ! -L "$env_file" ] || {
    load_runtime_env
    return
  }
  socket=${host_socket:-/var/run/docker.sock}
  root=${host_root:-$state_dir/periphery}
  root=$(canonical_path "$root")
  path_outside_repo "$root" 'P1_PERIPHERY_ROOT'
  case "$socket" in /*) ;; *) die 'P1_DOCKER_SOCKET must be absolute' ;; esac
  env_parent=$(dirname -- "$env_file")
  created_env_parent=0
  [ -d "$env_parent" ] || created_env_parent=1
  mkdir -p "$state_dir" "$root" "$env_parent"
  chmod 0700 "$state_dir" "$root"
  [ "$created_env_parent" -eq 0 ] || chmod 0700 "$env_parent"
  tmp_env=$(mktemp "$env_file.tmp.XXXXXX") ||
    die 'unable to create runtime environment temporary file'
  trap 'rm -f "$tmp_env"' EXIT HUP INT TERM
  umask 077
  mongo_password=$(openssl rand -hex 24)
  admin_password=$(openssl rand -hex 24)
  jwt_secret=$(openssl rand -hex 32)
  webhook_secret=$(openssl rand -hex 32)
  {
    printf '%s\n' 'P1_MONGO_ROOT_USERNAME=komodo-p1'
    printf 'P1_MONGO_ROOT_PASSWORD=%s\n' "$mongo_password"
    printf '%s\n' 'P1_DATABASE_NAME=komodo_p1_local'
    printf '%s\n' 'P1_INIT_ADMIN_USERNAME=p1-admin'
    printf 'P1_INIT_ADMIN_PASSWORD=%s\n' "$admin_password"
    printf 'P1_JWT_SECRET=%s\n' "$jwt_secret"
    printf 'P1_WEBHOOK_SECRET=%s\n' "$webhook_secret"
    printf '%s\n' 'P1_FIRST_SERVER_NAME=p1-local'
    printf 'P1_DOCKER_SOCKET=%s\n' "$socket"
    printf 'P1_PERIPHERY_ROOT=%s\n' "$root"
    printf 'P1_MONGO_PORT=%s\n' "${host_mongo_port:-27017}"
    printf 'P1_CORE_A_PORT=%s\n' "${host_core_a_port:-9120}"
    printf 'P1_CORE_B_PORT=%s\n' "${host_core_b_port:-9121}"
    printf 'P1_PERIPHERY_PORT=%s\n' "${host_periphery_port:-8120}"
  } >"$tmp_env"
  chmod 0600 "$tmp_env"
  mv "$tmp_env" "$env_file"
  trap - EXIT HUP INT TERM
  load_runtime_env
}

ensure_runtime_env() {
  if [ -e "$env_file" ] || [ -L "$env_file" ]; then
    load_runtime_env
  else
    create_runtime_env
  fi
}

doctor_checks() {
  if [ -e "$env_file" ] || [ -L "$env_file" ]; then
    load_runtime_env
    check_root=$runtime_root
    check_socket=$v_P1_DOCKER_SOCKET
    check_mongo_port=$v_P1_MONGO_PORT
    check_core_a_port=$v_P1_CORE_A_PORT
    check_core_b_port=$v_P1_CORE_B_PORT
    check_periphery_port=$v_P1_PERIPHERY_PORT
  else
    check_root=$(canonical_path "${host_root:-$state_dir/periphery}")
    check_socket=${host_socket:-/var/run/docker.sock}
    check_mongo_port=${host_mongo_port:-27017}
    check_core_a_port=${host_core_a_port:-9120}
    check_core_b_port=${host_core_b_port:-9121}
    check_periphery_port=${host_periphery_port:-8120}
  fi
  path_outside_repo "$check_root" 'P1_PERIPHERY_ROOT'
  case "$check_socket" in
    /*) ;;
    *) die 'P1_DOCKER_SOCKET must be absolute' ;;
  esac
  prepare_context
  (
  for tool in curl jq openssl python3 df docker; do
    command -v "$tool" >/dev/null 2>&1 || die "required tool missing: $tool"
  done
  requested_root=$check_root

  docker_cmd info >/dev/null || die 'Docker daemon is not running'
  docker_cmd compose \
    --project-name "$project" \
    --env-file "$example_env" \
    --file "$compose_file" version >/dev/null ||
    die 'Docker Compose is unavailable'
  docker_cmd buildx version >/dev/null || die 'Docker Buildx is unavailable'
  builder_output=$(docker_cmd buildx inspect --builder "$selected_context") ||
    die 'context-named Buildx builder is unavailable'
  builder_endpoints=$(printf '%s\n' "$builder_output" |
    awk '$1 == "Endpoint:" { print $2 }')
  [ -n "$builder_endpoints" ] ||
    die 'Buildx builder has no node endpoints'
  for builder_endpoint in $builder_endpoints; do
    [ "$builder_endpoint" = "$selected_context" ] ||
      die 'Buildx builder node endpoint does not match selected local context'
  done

  occupied_ports=$(python3 - \
    "$check_mongo_port" "$check_core_a_port" \
    "$check_core_b_port" "$check_periphery_port" <<'PY'
import socket
import sys
ports = [int(value) for value in sys.argv[1:]]
if len(set(ports)) != len(ports) or any(p < 1 or p > 65535 for p in ports):
    raise SystemExit("ports must be unique integers from 1 through 65535")
for port in ports:
    sock = socket.socket()
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        sock.bind(("127.0.0.1", port))
    except OSError:
        print(port)
    finally:
        sock.close()
PY
  )
  for port in $occupied_ports; do
    case "$port" in
      "$check_mongo_port") service=mongo; target=27017 ;;
      "$check_core_a_port") service=core-a; target=9120 ;;
      "$check_core_b_port") service=core-b; target=9120 ;;
      "$check_periphery_port") service=periphery; target=8120 ;;
      *) die "unexpected occupied port: $port" ;;
    esac
    container_id=$(docker_cmd ps \
      --filter "label=com.docker.compose.project=$project" \
      --filter "label=com.docker.compose.service=$service" \
      --format '{{.ID}}')
    [ -n "$container_id" ] ||
      die "port 127.0.0.1:$port is occupied outside the lab"
    binding=$(docker_cmd inspect --format \
      "{{(index (index .NetworkSettings.Ports \"$target/tcp\") 0).HostIp}}:{{(index (index .NetworkSettings.Ports \"$target/tcp\") 0).HostPort}}" \
      "$container_id")
    [ "$binding" = "127.0.0.1:$port" ] ||
      die "port 127.0.0.1:$port is not owned by $project/$service"
  done

  created_state=0
  created_root=0
  [ -d "$state_dir" ] || created_state=1
  [ -d "$requested_root" ] || created_root=1
  cleanup_doctor() {
    set +e
    rm -f "$requested_root/.p1-doctor-$$"
    rm -f "$state_dir/.p1-state-doctor-$$"
    [ "$created_root" -eq 0 ] || rmdir "$requested_root" 2>/dev/null
    [ "$created_state" -eq 0 ] || rmdir "$state_dir" 2>/dev/null
  }
  trap cleanup_doctor EXIT HUP INT TERM
  mkdir -p "$state_dir" "$requested_root"
  : >"$state_dir/.p1-state-doctor-$$"
  probe=.p1-doctor-$$
  : >"$requested_root/$probe"
  docker_cmd run --rm \
    --mount "type=bind,source=$requested_root,target=/p1-root" \
    "$mongo_image" sh -ec \
    "test -f /p1-root/$probe && echo daemon > /p1-root/$probe && test \"\$(cat /p1-root/$probe)\" = daemon"
  docker_cmd run --rm \
    --mount "type=bind,source=$check_socket,target=/p1-docker.sock" \
    "$mongo_image" sh -ec 'test -S /p1-docker.sock'
  available=$(df -Pk "$requested_root" | awk 'NR == 2 { print $4 }')
  printf 'Docker context: %s\nFree disk: %s KiB\n' "$selected_context" "$available"
  [ "${available:-0}" -ge 31457280 ] ||
    printf '%s\n' 'WARNING: less than 30 GiB free disk' >&2
  )
}

require_runtime() {
  load_runtime_env
  prepare_context
}

core_b_exists() {
  [ -n "$(docker_cmd ps --all \
    --filter "label=com.docker.compose.project=$project" \
    --filter 'label=com.docker.compose.service=core-b' \
    --format '{{.ID}}')" ]
}

deadline_exec() {
  deadline=$1
  shift
  remaining=$((deadline - $(date +%s)))
  [ "$remaining" -gt 0 ] || return 124
  python3 - "$remaining" "$@" <<'PY'
import os
import signal
import subprocess
import sys

timeout = int(sys.argv[1])
process = subprocess.Popen(sys.argv[2:], start_new_session=True)
try:
    return_code = process.wait(timeout=timeout)
except subprocess.TimeoutExpired:
    os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()
    raise SystemExit(124)
raise SystemExit(return_code)
PY
}

deadline_docker() {
  deadline=$1
  shift
  deadline_exec "$deadline" docker --context "$selected_context" "$@"
}

deadline_compose() {
  deadline=$1
  shift
  deadline_docker "$deadline" compose \
    --project-name "$project" \
    --env-file "$env_file" \
    --file "$compose_file" "$@"
}

request_timeout() {
  remaining=$(( $1 - $(date +%s) ))
  [ "$remaining" -gt 0 ] || return 124
  if [ "$remaining" -gt 5 ]; then
    printf '%s\n' 5
  else
    printf '%s\n' "$remaining"
  fi
}

wait_port_release() {
  port=$1
  deadline=$2
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if python3 - "$port" <<'PY'
import socket
import sys

sock = socket.socket()
sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
try:
    sock.bind(("127.0.0.1", int(sys.argv[1])))
except OSError:
    raise SystemExit(1)
finally:
    sock.close()
PY
    then
      return 0
    fi
    sleep 1
  done
  return 1
}

wait_lab_ports_release() {
  deadline=$(($(date +%s) + 30))
  for port in \
    "$v_P1_MONGO_PORT" "$v_P1_CORE_A_PORT" \
    "$v_P1_CORE_B_PORT" "$v_P1_PERIPHERY_PORT"
  do
    wait_port_release "$port" "$deadline" ||
      die "lab port 127.0.0.1:$port was not released"
  done
}

toolbox_ping() {
  deadline=$1
  deadline_compose "$deadline" --profile tools run --rm --no-deps toolbox \
    sh -ec 'exec mongosh --quiet \
      --host mongo --port 27017 \
      --username "$MONGO_INITDB_ROOT_USERNAME" \
      --password "$MONGO_INITDB_ROOT_PASSWORD" \
      --authenticationDatabase admin --eval \
      '\''const lab = db.getSiblingDB(process.env.P1_DATABASE_NAME); quit(lab.runCommand({ ping: 1 }).ok == 1 ? 0 : 1)'\'''
}

poll_version() {
  url=$1
  deadline=$2
  attempts=0
  while [ "$attempts" -lt 60 ] && [ "$(date +%s)" -lt "$deadline" ]; do
    limit=$(request_timeout "$deadline") || return 1
    if curl --silent --show-error --fail \
      --connect-timeout "$limit" --max-time "$limit" "$url" >/dev/null
    then
      return 0
    fi
    attempts=$((attempts + 1))
    remaining=$((deadline - $(date +%s)))
    [ "$remaining" -gt 0 ] || return 1
    if [ "$remaining" -gt 2 ]; then
      sleep 2
    else
      sleep "$remaining"
    fi
  done
  return 1
}

recent_logs() {
  log_file=$(mktemp)
  log_deadline=$(($(date +%s) + 5))
  deadline_compose "$log_deadline" logs --tail 200 \
    mongo core-a periphery >"$log_file" 2>&1 || true
  python3 - "$env_file" "$log_file" <<'PY'
import sys

env_path, log_path = sys.argv[1:]
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
with open(log_path, encoding="utf-8", errors="replace") as log:
    output = log.read()
for secret in secrets:
    output = output.replace(secret, "<redacted>")
sys.stderr.write(output)
PY
  rm -f "$log_file"
}

login_core() {
  port=$1
  deadline=$2
  limit=$(request_timeout "$deadline") || die 'local admin login timed out'
  login_file=$(mktemp)
  if ! printf '{"username":"%s","password":"%s"}' \
    "$v_P1_INIT_ADMIN_USERNAME" "$v_P1_INIT_ADMIN_PASSWORD" |
    curl --silent --show-error --fail --connect-timeout "$limit" \
      --max-time "$limit" \
      --header 'Content-Type: application/json' --data-binary @- \
      "http://127.0.0.1:$port/auth/login/LoginLocalUser" >"$login_file"
  then
    rm -f "$login_file"
    die 'local admin login failed'
  fi
  jwt=$(jq -er 'select(.type == "Jwt") | .data.jwt | select(type == "string" and length > 0)' "$login_file") || {
    rm -f "$login_file"
    die 'local admin login did not return Jwt'
  }
  rm -f "$login_file"
}

authenticated_request() {
  port=$1
  route=$2
  output=$3
  body=$4
  deadline=$5
  limit=$(request_timeout "$deadline") || return 124
  {
    printf 'header = "Authorization: Bearer %s"\n' "$jwt"
    printf '%s\n' 'header = "Content-Type: application/json"'
    printf 'data = "%s"\n' "$(printf '%s' "$body" | sed 's/"/\\"/g')"
  } | curl --silent --show-error --fail --connect-timeout "$limit" \
    --max-time "$limit" \
    --config - "http://127.0.0.1:$port$route" >"$output"
}

check_stats() {
  port=$1
  deadline=$2
  stats_file=$(mktemp)
  if ! authenticated_request "$port" /read/GetSystemStats "$stats_file" \
    '{"server":"p1-local"}' "$deadline"
  then
    rm -f "$stats_file"
    return 1
  fi
  jq -e '.cpu_perc | type == "number"' "$stats_file" >/dev/null &&
    jq -e '.mem_total_gb > 0 and has("polling_rate") and .refresh_ts > 0' \
      "$stats_file" >/dev/null || {
    rm -f "$stats_file"
    return 1
  }
  rm -f "$stats_file"
}

poll_stats() {
  port=$1
  deadline=$2
  attempts=0
  while [ "$attempts" -lt 60 ] && [ "$(date +%s)" -lt "$deadline" ]; do
    if check_stats "$port" "$deadline"; then
      return 0
    fi
    attempts=$((attempts + 1))
    remaining=$((deadline - $(date +%s)))
    [ "$remaining" -gt 0 ] || return 1
    if [ "$remaining" -gt 2 ]; then
      sleep 2
    else
      sleep "$remaining"
    fi
  done
  return 1
}

check_daemon_ids() {
  deadline=$1
  list_file=$(mktemp)
  if ! authenticated_request "$v_P1_CORE_A_PORT" \
    /read/ListDockerContainers "$list_file" '{"server":"p1-local"}' \
    "$deadline"
  then
    rm -f "$list_file"
    die 'ListDockerContainers request failed'
  fi
  for service in mongo core-a periphery; do
    expected_id=$(deadline_compose "$deadline" ps -q "$service")
    if [ -z "$expected_id" ]; then
      rm -f "$list_file"
      die "missing Compose container: $service"
    fi
    jq -e --arg id "$expected_id" '.. | objects | .id? // empty | select(. == $id)' \
      "$list_file" >/dev/null || {
      rm -f "$list_file"
      die 'Periphery Docker API does not match the selected daemon'
    }
  done
  rm -f "$list_file"
}

export_core_public_key() {
  deadline=$(($(date +%s) + 120))
  poll_version "http://127.0.0.1:$v_P1_CORE_A_PORT/version" "$deadline" ||
    die 'Core A readiness timed out before public-key export'
  login_core "$v_P1_CORE_A_PORT" "$deadline"
  info_file=$(mktemp)
  if ! authenticated_request "$v_P1_CORE_A_PORT" /read/GetCoreInfo \
    "$info_file" '{}' "$deadline"
  then
    rm -f "$info_file"
    die 'GetCoreInfo failed during public-key export'
  fi
  public_key=$(jq -er '.public_key | select(type == "string" and length > 0)' \
    "$info_file") || {
    rm -f "$info_file"
    die 'GetCoreInfo returned no public key'
  }
  rm -f "$info_file"
  deadline_compose "$deadline" --profile tools run --rm --no-deps toolbox \
    sh -ec 'umask 077; printf "%s" "$1" > /config/keys/core.pub; test -s /config/keys/core.pub' \
    sh "$public_key" >/dev/null || die 'failed to export Core public key'
}

wait_base() {
  deadline=$(($(date +%s) + 120))
  mongo_id=$(deadline_compose "$deadline" ps -q mongo)
  [ -n "$mongo_id" ] || die 'Mongo container is absent'
  [ "$(deadline_docker "$deadline" inspect \
    --format '{{.State.Health.Status}}' "$mongo_id")" = \
    healthy ] || die 'Mongo healthcheck is not healthy'
  toolbox_ping "$deadline" >/dev/null || die 'authenticated Mongo ping failed'
  poll_version "http://127.0.0.1:$v_P1_CORE_A_PORT/version" "$deadline" || {
    recent_logs
    die 'Core A readiness timed out'
  }
  poll_version "http://127.0.0.1:$v_P1_PERIPHERY_PORT/version" "$deadline" || {
    recent_logs
    die 'Periphery readiness timed out'
  }
  [ "$(date +%s)" -lt "$deadline" ] || {
    recent_logs
    die 'base readiness timed out'
  }
  login_core "$v_P1_CORE_A_PORT" "$deadline"
  poll_stats "$v_P1_CORE_A_PORT" "$deadline" || {
    recent_logs
    die 'GetSystemStats readiness timed out'
  }
  check_daemon_ids "$deadline"
  [ "$(date +%s)" -lt "$deadline" ] || die 'base readiness timed out'
}

preflight_js='const lab = db.getSiblingDB(process.env.P1_DATABASE_NAME);
const counts = {
  in_progress_updates: lab.getCollection("Update").countDocuments({ status: "InProgress" }),
  procedure_schedules: lab.getCollection("Procedure").countDocuments({ "config.schedule_enabled": { $ne: false }, "config.schedule": { $type: "string", $ne: "" } }),
  action_schedules: lab.getCollection("Action").countDocuments({ "config.schedule_enabled": { $ne: false }, "config.schedule": { $type: "string", $ne: "" } }),
  startup_actions: lab.getCollection("Action").countDocuments({ "config.run_at_startup": true }),
  gitops_stacks: lab.getCollection("Stack").countDocuments({ "config.auto_deploy_git_updates": true, "config.files_on_host": { $ne: true }, $and: [{ $or: [{ "config.repo": { $type: "string", $ne: "" } }, { "config.linked_repo": { $type: "string", $ne: "" } }] }, { $or: [{ "config.commit": "" }, { "config.commit": { $exists: false } }] }] }),
  gitops_resource_syncs: lab.getCollection("ResourceSync").countDocuments({ "config.auto_apply_updates": true, "config.files_on_host": { $ne: true }, $and: [{ $or: [{ "config.repo": { $type: "string", $ne: "" } }, { "config.linked_repo": { $type: "string", $ne: "" } }] }, { $or: [{ "config.commit": "" }, { "config.commit": { $exists: false } }] }] })
};
print(JSON.stringify(counts));
quit(Object.values(counts).every((count) => count === 0) ? 0 : 42);'

cross_core_preflight() {
  preflight_file=$(mktemp)
  set +e
  compose_cmd --profile tools run --rm --no-deps toolbox \
    sh -ec 'exec mongosh --quiet \
      --host mongo --port 27017 \
      --username "$MONGO_INITDB_ROOT_USERNAME" \
      --password "$MONGO_INITDB_ROOT_PASSWORD" \
      --authenticationDatabase admin --eval "$1"' sh \
      "$preflight_js" >"$preflight_file"
  preflight_rc=$?
  set -e
  if [ "$preflight_rc" -eq 42 ]; then
    blocker=$(jq -r 'to_entries[] | select(.value != 0) | .key' "$preflight_file" |
      paste -sd, -)
    rm -f "$preflight_file"
    die "cross-Core preflight refused: $blocker"
  fi
  [ "$preflight_rc" -eq 0 ] || {
    rm -f "$preflight_file"
    die 'cross-Core preflight query failed'
  }
  jq -e 'keys == ["action_schedules","gitops_resource_syncs","gitops_stacks","in_progress_updates","procedure_schedules","startup_actions"] and all(.[]; . == 0)' \
    "$preflight_file" >/dev/null || {
    rm -f "$preflight_file"
    die 'cross-Core preflight returned an invalid result'
  }
  rm -f "$preflight_file"
}

cross_cleanup() {
  cleanup_rc=$?
  set +e
  compose_cmd unpause core-a >/dev/null 2>&1
  if [ "${core_b_attempted:-0}" -eq 1 ]; then
    compose_cmd --profile cross-core rm --stop --force core-b >/dev/null 2>&1
  fi
  exit "$cleanup_rc"
}

case "$command" in
  'doctor')
    doctor_checks
    ;;
  'config')
    env_file=$example_env
    prepare_context
    compose_cmd --profile cross-core --profile tools config --quiet
    printf '%s\n' 'Services:'
    compose_cmd --profile cross-core --profile tools config --services
    printf '%s\n' 'Profiles:'
    compose_cmd --profile cross-core --profile tools config --profiles
    ;;
  'build')
    doctor_checks
    ensure_runtime_env
    compose_cmd build core-a periphery
    ;;
  'up')
    doctor_checks
    core_b_exists && die 'Core B already exists'
    ensure_runtime_env
    prepare_context
    compose_cmd up -d --build mongo core-a
    export_core_public_key
    compose_cmd up -d --build --no-deps periphery
    wait_base
    ;;
  'wait')
    require_runtime
    wait_base
    ;;
  'cross-core-up')
    require_runtime
    core_b_exists && die 'Core B already exists'
    wait_base
    cross_core_preflight
    core_b_attempted=0
    trap cross_cleanup EXIT HUP INT TERM
    compose_cmd pause core-a
    cross_core_preflight
    core_b_attempted=1
    compose_cmd --profile cross-core up -d --no-deps core-b
    core_b_deadline=$(($(date +%s) + 120))
    poll_version "http://127.0.0.1:$v_P1_CORE_B_PORT/version" \
      "$core_b_deadline" || {
      recent_logs
      die 'Core B readiness timed out'
    }
    login_core "$v_P1_CORE_B_PORT" "$core_b_deadline"
    poll_stats "$v_P1_CORE_B_PORT" "$core_b_deadline" ||
      die 'Core B GetSystemStats readiness timed out'
    [ "$(date +%s)" -lt "$core_b_deadline" ] ||
      die 'Core B readiness timed out'
    compose_cmd unpause core-a
    core_b_attempted=0
    trap - EXIT HUP INT TERM
    ;;
  'cross-core-down')
    require_runtime
    compose_cmd --profile cross-core rm --stop --force core-b
    port_deadline=$(($(date +%s) + 30))
    wait_port_release "$v_P1_CORE_B_PORT" "$port_deadline" ||
      die "Core B port 127.0.0.1:$v_P1_CORE_B_PORT was not released"
    ;;
  'status')
    require_runtime
    compose_cmd --profile cross-core --profile tools ps --all \
      --format json | jq -r '
        .[] |
        [
          .Service,
          .State,
          (.Health // ""),
          ([.Publishers[]? |
            select(.URL == "127.0.0.1") |
            "\(.URL):\(.PublishedPort)->\(.TargetPort)"] | join(","))
        ] | @tsv'
    ;;
  'down')
    require_runtime
    compose_cmd --profile cross-core --profile tools down --remove-orphans
    wait_lab_ports_release
    ;;
  'reset')
    require_runtime
    compose_cmd --profile cross-core --profile tools down --volumes --remove-orphans
    wait_lab_ports_release
    ;;
esac
