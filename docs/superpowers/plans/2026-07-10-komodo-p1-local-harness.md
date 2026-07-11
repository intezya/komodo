# Komodo P1 Local Performance Lab Implementation Plan

> **For agentic workers:** Execute this plan task-by-task with fresh implementation agents. Each task requires test-first evidence, spec-compliance review, and code-quality review before the next task starts.

**Goal:** Provide one safe, reproducible local lab that builds the current checkout and runs MongoDB, Core A, inbound Periphery, and an opt-in Core B so the four P1 performance plans can perform functional and integration checks before their staging or remote acceptance gates.

**Architecture:** A fixed `komodo-p1-local` Compose project builds the existing AIO Core and Periphery Dockerfiles, runs a digest-pinned MongoDB, stores runtime secrets outside the checkout, and exposes only loopback ports. The default stack contains MongoDB, Core A, and Periphery. `cross-core-up` closes the Core-startup race by checking MongoDB, pausing Core A, checking again, starting Core B with `--no-deps`, proving authenticated Core B to Periphery traffic, and always unpausing Core A. A separate verifier exercises failure paths, persistence, reset isolation, and emits a secret-free JSON proof under `target/`.

**Tech Stack:** Docker Compose v2, Docker Buildx/BuildKit, POSIX shell, `jq`, Python 3 for portable path/port probes, MongoDB 8.0.26, existing Rust 1.95 AIO images, Axum/Mogh Auth APIs, Core and Periphery public `/version` endpoints.

**Approved design:** `docs/superpowers/specs/2026-07-10-komodo-p1-local-harness-design.md`

---

## Scope and fixed decisions

This is one preparatory lab PR. It does not implement any production
optimization from Plans 1–4.

The MongoDB image is frozen for this checkpoint:

```text
mongo:8.0.26@sha256:ffa440e8d62533e24a67696ae1bbb46e610ebb3167d65abd122b496ae06d28e6
```

Registry inspection on 2026-07-10 resolved:

```text
linux/amd64    sha256:721f8fe7ae88f6acee8c163a358f726cef6dfc4181b9d3ca77212a0cef6b781c
linux/arm64/v8 sha256:9f06aa22d02efb6cee9e6e45c11eb57ed830c4fb8889fba531725230d74419eb
```

Changing the MongoDB tag or digest is a separate reviewed dependency change.
Do not silently follow `mongo:8`, `mongo:latest`, or the newer 8.2 line during
this implementation.

The lab uses inbound Periphery over HTTP inside the private Compose network,
with Noise public-key authentication. Do not define
`PERIPHERY_CORE_ADDRESS` or `PERIPHERY_CORE_ADDRESSES`.

Core host identities are deliberately distinct and browser reachable:

```text
Core A KOMODO_HOST=http://127.0.0.1:${P1_CORE_A_PORT}
Core B KOMODO_HOST=http://localhost:${P1_CORE_B_PORT}
```

Periphery indexes inbound channels by the hostname portion, so using the same
hostname with different ports is invalid.

The wrapper is the only supported orchestration entrypoint. Every Compose call
must include:

```text
--project-name komodo-p1-local
--env-file <explicit file>
--file <repo>/p1.local.compose.yaml
```

No task may:

- modify `dev.compose.yaml`, `expose.compose.yaml`, or old deployment Compose
  files;
- add a fixture seeder, profiler, Playwright, HAR collector, or fault proxy;
- change Rust or React production behavior;
- read production or QA credentials;
- publish images, write GHCR, or dispatch GitHub Actions;
- make Core B persistent or start it by default;
- add a root `.dockerignore`;
- delete unrelated Docker objects, volumes, caches, or files;
- claim macOS, virtualized, or QEMU timings are final performance evidence.

## File map

**Checkpoint 1 — static model and source-build boundary**

- Create: `scripts/performance/check-p1-local-lab.sh`
- Create: `p1.local.compose.yaml`
- Create: `compose/p1.local.env.example`
- Create: `bin/core/aio.Dockerfile.dockerignore`
- Create: `bin/periphery/aio.Dockerfile.dockerignore`

**Checkpoint 2 — lifecycle and authenticated cross-Core behavior**

- Create: `scripts/performance/p1-local-wrapper.test.sh`
- Create: `scripts/performance/p1-local.sh`
- Modify: `scripts/performance/check-p1-local-lab.sh`

**Checkpoint 3 — real BuildKit/runtime proof and operator handoff**

- Create: `scripts/performance/check-p1-local-build-context.sh`
- Create: `scripts/performance/verify-p1-local-runtime.sh`
- Create: `docs/performance/p1-local-lab.md`
- Modify: `docs/superpowers/specs/2026-07-10-komodo-p1-local-harness-design.md`

## Ordered commit checkpoints

1. `feat: define P1 local lab stack` — Tasks 1–2. Daemon-free contract and
   Compose rendering must pass before commit.
2. `feat: add P1 local lab lifecycle` — Tasks 3–4. Fake-tool failure tests and
   all shell/static checks must pass before commit.
3. `test: verify P1 local lab runtime` — Task 5. Requires a running local
   Docker engine and a complete secret-free proof artifact.
4. `docs: document P1 local performance lab` — Task 6. Final static and runtime
   gates must be repeated after documentation and spec refinements.

Every commit stays on branch `perf-p1-local-harness`. Any future PR must target
only `intezya/komodo:main`.

---

### Task 1: Write the failing Compose and build-context contract

**Files:**

- Create: `scripts/performance/check-p1-local-lab.sh`
- Test: `scripts/performance/check-p1-local-lab.sh`

- [ ] **Step 1: Create the contract harness before any implementation file**

The directory does not exist in the current checkout. Create it first with
`rtk mkdir -p scripts/performance`, add the file with `apply_patch`, then run
`rtk chmod +x scripts/performance/check-p1-local-lab.sh`.

Create a POSIX shell script with `set -eu`, repository-root discovery relative
to the script, and these ordered file checks:

```text
p1.local.compose.yaml
compose/p1.local.env.example
bin/core/aio.Dockerfile.dockerignore
bin/periphery/aio.Dockerfile.dockerignore
```

The first missing path must fail with exactly:

```text
FAIL: missing <path>
```

Do not require `p1-local.sh` yet; Task 3 extends the same contract after the
Compose checkpoint is green.

- [ ] **Step 2: Add the rendered Compose validator shape**

After file presence, render all opt-in profiles to a temporary JSON file:

```sh
docker compose \
  --project-name komodo-p1-local \
  --env-file compose/p1.local.env.example \
  --profile cross-core \
  --profile tools \
  --file p1.local.compose.yaml \
  config --format json
```

Use `jq -e`; do not use broad grep as the authoritative model check. Assert:

- service keys, sorted, are exactly
  `core-a`, `core-b`, `mongo`, `periphery`, `toolbox`;
- the profile union is exactly `cross-core`, `tools`;
- Core A and Core B use the repository root build context and
  `bin/core/aio.Dockerfile`;
- Periphery uses the repository root build context and
  `bin/periphery/aio.Dockerfile`;
- Mongo and toolbox use the exact pinned image reference;
- rendered example ports are exactly `(127.0.0.1,27017,27017)`,
  `(127.0.0.1,9120,9120)`, `(127.0.0.1,9121,9120)`, and
  `(127.0.0.1,8120,8120)` as `(host_ip,published,target)` tuples;
- all services attach only to network `lab`;
- Mongo mounts `mongo-data` and `mongo-config`, while Core A/B and Periphery
  share `keys`;
- Core A/B database credentials and database name are identical;
- Core A/B have different `KOMODO_HOST` and `KOMODO_DATABASE_APP_NAME` values;
- Core A/B both enable local auth, disable user registration and bootstrap
  resources, and use the same bounded monitoring/resource-poll settings;
- Core B has no first-server or initial-admin environment variables;
- Periphery has explicit inbound HTTP variables and no outbound variables;
- only Periphery mounts the Docker socket and `/proc`;
- Core B and toolbox have `restart: "no"`;
- Mongo, Core A/B, and Periphery healthcheck commands and all interval,
  timeout, retry, and start-period values match the model below;
- Core A waits for healthy Mongo, Periphery waits for healthy Core A, and Core B
  declares healthy Mongo/Core A/Periphery dependencies;
- no image, host, or database value contains `ghcr.io`, `ferretdb`,
  `mongodb+srv`, `komo.do`, or a production-looking hostname.

Render with both profiles because plain `docker compose config` omits profiled
services.

- [ ] **Step 3: Add Dockerfile-specific ignore assertions**

For each AIO ignore file, require:

- deny-all `*` as the first effective rule;
- explicit allowlist entries for only the paths copied by its Dockerfile;
- terminal deny rules for `.git`, `.worktrees`, `.dev`, `target`, all nested
  `node_modules`, all nested `dist`, `.env*`, `.npmrc`, `.yarnrc*`, and nested
  equivalents;
- no later negation that can re-include a denied secret path.

The terminal deny block is required after allowlist negations because
`!ui/**` would otherwise re-include `ui/.env.development`.

- [ ] **Step 4: Run the test and record RED**

Run:

```sh
rtk sh -n scripts/performance/check-p1-local-lab.sh
rtk sh scripts/performance/check-p1-local-lab.sh
```

Expected:

```text
FAIL: missing p1.local.compose.yaml
```

The second command must exit nonzero for that reason, not because of shell
syntax, a missing `jq`, or an unavailable Docker daemon. `docker compose
config` itself is daemon-free.

Do not commit the RED-only state.

---

### Task 2: Define the digest-pinned source-built Compose stack

**Files:**

- Create: `p1.local.compose.yaml`
- Create: `compose/p1.local.env.example`
- Create: `bin/core/aio.Dockerfile.dockerignore`
- Create: `bin/periphery/aio.Dockerfile.dockerignore`
- Verify: `scripts/performance/check-p1-local-lab.sh`

- [ ] **Step 1: Add the example environment contract**

Create `compose/p1.local.env.example` with exactly these keys:

```dotenv
P1_MONGO_ROOT_USERNAME=komodo-p1
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
P1_PERIPHERY_PORT=8120
```

Do not put `P1_STATE_DIR`, `P1_ENV_FILE`, or `P1_DOCKER_CONTEXT` in this file;
they control the host wrapper rather than container configuration.

- [ ] **Step 2: Create the Compose objects and Mongo service**

Do not add top-level `name:` or any `container_name`. Define:

```yaml
networks:
  lab: {}

volumes:
  mongo-data: {}
  mongo-config: {}
  keys: {}
```

The `mongo` service must use:

```yaml
image: mongo:8.0.26@sha256:ffa440e8d62533e24a67696ae1bbb46e610ebb3167d65abd122b496ae06d28e6
restart: unless-stopped
ports:
  - "127.0.0.1:${P1_MONGO_PORT:-27017}:27017"
```

Pass `P1_MONGO_ROOT_USERNAME` and `P1_MONGO_ROOT_PASSWORD` to the official
`MONGO_INITDB_ROOT_*` variables, attach the two Mongo volumes and `lab`
network, add label `komodo.skip: "true"`, and use an authenticated `mongosh`
ping healthcheck. Escape container-side variables as `$$` so Compose does not
interpolate them on the host. Use interval 2 seconds, timeout 5 seconds, 30
retries, and 10-second start period. The `mongosh` command must pass
`--authenticationDatabase admin` because the root user is created in `admin`.

- [ ] **Step 3: Add Core A**

Core A must:

- build `bin/core/aio.Dockerfile` with context `.`;
- use image name `komodo-p1-local-core:dev`;
- set `init: true`, `restart: unless-stopped`, network `lab`, and
  `keys:/config/keys`;
- depend only on healthy Mongo;
- publish `127.0.0.1:${P1_CORE_A_PORT:-9120}:9120`;
- healthcheck `GET http://127.0.0.1:9120/version` with image-provided `curl`.

Use interval 2 seconds, timeout 5 seconds, 30 retries, and a 10-second start
period for Core A.

Set this exact semantic environment:

```text
KOMODO_HOST=http://127.0.0.1:${P1_CORE_A_PORT:-9120}
KOMODO_PORT=9120
KOMODO_BIND_IP=0.0.0.0
KOMODO_DATABASE_ADDRESS=mongo:27017
KOMODO_DATABASE_USERNAME=${P1_MONGO_ROOT_USERNAME}
KOMODO_DATABASE_PASSWORD=${P1_MONGO_ROOT_PASSWORD}
KOMODO_DATABASE_DB_NAME=${P1_DATABASE_NAME}
KOMODO_DATABASE_APP_NAME=komodo_p1_core_a
KOMODO_PRIVATE_KEY=file:/config/keys/core.key
KOMODO_FIRST_SERVER_NAME=${P1_FIRST_SERVER_NAME}
KOMODO_FIRST_SERVER_ADDRESS=http://periphery:8120
KOMODO_LOCAL_AUTH=true
KOMODO_DISABLE_USER_REGISTRATION=true
KOMODO_DISABLE_INIT_RESOURCES=true
KOMODO_INIT_ADMIN_USERNAME=${P1_INIT_ADMIN_USERNAME}
KOMODO_INIT_ADMIN_PASSWORD=${P1_INIT_ADMIN_PASSWORD}
KOMODO_JWT_SECRET=${P1_JWT_SECRET}
KOMODO_WEBHOOK_SECRET=${P1_WEBHOOK_SECRET}
KOMODO_MONITORING_INTERVAL=15-sec
KOMODO_RESOURCE_POLL_INTERVAL=1-day
```

Use `KOMODO_FIRST_SERVER_ADDRESS`, not the legacy alias. Core A may become
healthy before Periphery; the monitor loop must connect after Periphery starts.

- [ ] **Step 4: Add inbound Periphery**

Periphery must:

- build `bin/periphery/aio.Dockerfile` with context `.`;
- use image `komodo-p1-local-periphery:dev`;
- set `init: true`, `restart: unless-stopped`, and network `lab`;
- depend on healthy Core A so `core.pub` exists;
- publish `127.0.0.1:${P1_PERIPHERY_PORT:-8120}:8120`;
- share `keys:/config/keys`;
- bind `${P1_DOCKER_SOCKET}` to `/var/run/docker.sock`;
- bind `/proc` read-only;
- bind `${P1_PERIPHERY_ROOT}` to the identical target path;
- healthcheck plain HTTP `GET /version`.

Use interval 2 seconds, timeout 5 seconds, 30 retries, and a 5-second start
period for Periphery.

Set:

```text
PERIPHERY_SERVER_ENABLED=true
PERIPHERY_PORT=8120
PERIPHERY_BIND_IP=0.0.0.0
PERIPHERY_SSL_ENABLED=false
PERIPHERY_PRIVATE_KEY=file:/config/keys/periphery.key
PERIPHERY_CORE_PUBLIC_KEYS=file:/config/keys/core.pub
PERIPHERY_ROOT_DIRECTORY=${P1_PERIPHERY_ROOT}
PERIPHERY_INCLUDE_DISK_MOUNTS=/etc/hostname
```

Do not follow the misspelled `PERIHERY_SERVER_ENABLED` comment in the sample
config; the Rust environment field is `PERIPHERY_SERVER_ENABLED`.

- [ ] **Step 5: Add Core B and toolbox profiles**

Core B uses profile `cross-core`, the same image/build, network, Mongo, JWT,
webhook secret, private key, key volume, API bind/port, local-auth policy, and
monitoring settings as Core A. Its environment must include:

```text
restart: "no"
KOMODO_PORT=9120
KOMODO_BIND_IP=0.0.0.0
KOMODO_HOST=http://localhost:${P1_CORE_B_PORT:-9121}
KOMODO_DATABASE_ADDRESS=mongo:27017
KOMODO_DATABASE_USERNAME=${P1_MONGO_ROOT_USERNAME}
KOMODO_DATABASE_PASSWORD=${P1_MONGO_ROOT_PASSWORD}
KOMODO_DATABASE_DB_NAME=${P1_DATABASE_NAME}
KOMODO_DATABASE_APP_NAME=komodo_p1_core_b
KOMODO_PRIVATE_KEY=file:/config/keys/core.key
KOMODO_LOCAL_AUTH=true
KOMODO_DISABLE_USER_REGISTRATION=true
KOMODO_DISABLE_INIT_RESOURCES=true
KOMODO_JWT_SECRET=${P1_JWT_SECRET}
KOMODO_WEBHOOK_SECRET=${P1_WEBHOOK_SECRET}
KOMODO_MONITORING_INTERVAL=15-sec
KOMODO_RESOURCE_POLL_INTERVAL=1-day
```

Publish host port 9121 to container port 9120. Declare healthy dependencies on
Mongo, Core A, and Periphery for model documentation, but the wrapper must use
`--no-deps` while Core A is paused. Do not pass Core A's first-server or
initial-admin variables to Core B. Give Core B the same internal `/version`
healthcheck and timing values as Core A.

Toolbox uses profile `tools`, the exact pinned Mongo image, network `lab`, no
published ports, no Docker socket, `restart: "no"`, and `command: [sleep,
infinity]`. It depends on healthy Mongo and receives exactly:

```text
MONGO_INITDB_ROOT_USERNAME=${P1_MONGO_ROOT_USERNAME}
MONGO_INITDB_ROOT_PASSWORD=${P1_MONGO_ROOT_PASSWORD}
P1_DATABASE_NAME=${P1_DATABASE_NAME}
```

- [ ] **Step 6: Add allowlist-first Dockerfile-specific ignores**

Both ignore files begin with `*`, then re-include only the inputs copied by the
owning AIO Dockerfile and their parent directories. Core needs Cargo manifests,
`lib`, Core Rust/TS clients, Periphery client, Core and CLI bins, `xtask`, `ui`,
`config/core.config.toml`, and `bin/entrypoint.sh`. Periphery needs Cargo
manifests, `lib`, Core Rust client, Periphery client, Periphery bin, `xtask`,
and `bin/entrypoint.sh`.

End both files with explicit terminal denies for Git metadata, worktrees,
`.dev`, target trees, dependency/build output, all `.env*` variants, `.npmrc`,
and `.yarnrc*`. Do not create or modify a root `.dockerignore`.

- [ ] **Step 7: Run GREEN daemon-free validation**

Run:

```sh
rtk sh -n scripts/performance/check-p1-local-lab.sh
rtk sh scripts/performance/check-p1-local-lab.sh
rtk docker compose \
  --project-name komodo-p1-local \
  --env-file compose/p1.local.env.example \
  --profile cross-core \
  --profile tools \
  --file p1.local.compose.yaml \
  config --quiet
rtk git diff --check
```

Expected final contract output:

```text
P1 local lab contract OK
```

- [ ] **Step 8: Commit checkpoint 1**

```sh
rtk git add \
  p1.local.compose.yaml \
  compose/p1.local.env.example \
  bin/core/aio.Dockerfile.dockerignore \
  bin/periphery/aio.Dockerfile.dockerignore \
  scripts/performance/check-p1-local-lab.sh
rtk git commit -m "feat: define P1 local lab stack"
```

---

### Task 3: Specify the lifecycle wrapper through black-box failure tests

**Files:**

- Create: `scripts/performance/p1-local-wrapper.test.sh`
- Modify: `scripts/performance/check-p1-local-lab.sh`
- Test: `scripts/performance/p1-local-wrapper.test.sh`

- [ ] **Step 1: Extend the static contract before creating the wrapper**

Require `scripts/performance/p1-local.sh`, the fixed project name, explicit
`--env-file`, the ten approved commands, and the absence of raw `compose
config` output. Do not parse runtime secrets from a rendered configuration.

- [ ] **Step 2: Create a fake-tool black-box test**

The test creates a temporary `PATH` containing fake `docker`, `curl`,
`openssl`, and `df` executables. Fake Docker records every argument in a log and
returns controlled context/Compose results.

After adding the test with `apply_patch`, run
`rtk chmod +x scripts/performance/p1-local-wrapper.test.sh`.

Cover at least:

- unknown command and extra arguments exit 64 before Docker;
- `reset` without exactly `--yes` prints `reset requires --yes` and never calls
  Docker;
- `ssh://`, `tcp://`, and relative context endpoints fail before `info`, pull,
  run, or Compose mutation;
- generated environment uses a temporary file, atomic rename, and mode 0600;
- an existing env that is a symlink, non-regular file, foreign-owned file, or
  not exactly mode 0600 is rejected before Docker;
- runtime commands reject `compose/p1.local.env.example`;
- ambient Compose interpolation variables cannot override the selected env
  file, including sentinel secret values;
- state/env/Periphery paths inside the checkout are rejected;
- `config` output contains only allowlisted service/profile names and never the
  injected sentinel Mongo/admin/JWT/webhook values;
- every Compose call has fixed project, file, and env arguments;
- `up` uses `--build` with only the three base services and refuses while Core B
  already exists;
- `wait` performs an authenticated Mongo ping, bounded `/version` retries, the
  local-admin JWT flow, and `GetSystemStats` without placing passwords or JWTs
  in recorded argv;
- authenticated `ListDockerContainers` through Core A returns the exact
  Mongo/Core A/Periphery container IDs reported by the selected Docker context,
  proving Periphery's mounted socket reaches that same daemon;
- every one-shot toolbox trace uses `--profile tools run --rm --no-deps
  toolbox`, authenticates against `admin`, and selects `P1_DATABASE_NAME`;
- the fake trace for `cross-core-up` is first preflight, pause Core A, second
  preflight, `up -d --no-deps core-b`, Core B auth/stats, then unpause;
- each nonzero named preflight count independently refuses Core B, including
  opt-in GitOps Stack and Resource Sync counts from the current `main` model;
- injected failure at the second preflight, Core B start, and Core B readiness
  always unpauses Core A and removes any attempted Core B;
- `doctor`, `build`, and `up` preflight failures leave no runtime env, state
  directory, or Compose mutation, and a repeated `cross-core-up` refuses before
  preflight when Core B already exists;
- an ambient `BUILDX_BUILDER` is ignored and a builder whose node endpoint is
  not the selected local context is rejected before build;
- `down` never includes `--volumes`;
- `reset --yes` includes `--volumes` and `--remove-orphans`;
- `cross-core-down` removes only Core B.

- [ ] **Step 3: Run and record RED**

```sh
rtk sh -n scripts/performance/p1-local-wrapper.test.sh
rtk sh scripts/performance/p1-local-wrapper.test.sh
```

Expected: nonzero with `missing scripts/performance/p1-local.sh` or the first
unimplemented lifecycle assertion.

Do not weaken the test to make an incomplete wrapper green.

---

### Task 4: Implement safe lifecycle, readiness, and Core B admission

**Files:**

- Create: `scripts/performance/p1-local.sh`
- Modify: `scripts/performance/check-p1-local-lab.sh`
- Verify: `scripts/performance/p1-local-wrapper.test.sh`

- [ ] **Step 1: Implement strict argument and path handling**

Use `#!/usr/bin/env sh` and `set -eu`. Resolve the repository from the script
path. Default state to
`${XDG_STATE_HOME:-$HOME/.local/state}/komodo-p1-local`, canonicalize paths with
Python 3, and reject runtime state, env, and Periphery roots inside the
checkout.

After adding the wrapper with `apply_patch`, run
`rtk chmod +x scripts/performance/p1-local.sh`.

Accepted commands are exactly:

```text
doctor config build up wait cross-core-up cross-core-down status down reset
```

Only `reset --yes` accepts an argument. Unknown commands or extra arguments
exit 64.

- [ ] **Step 2: Generate the runtime environment without sourcing it**

Use `umask 077`, `openssl rand -hex 24` for Mongo/admin passwords, and
`openssl rand -hex 32` for JWT/webhook secrets. Write to a temporary sibling
file, `chmod 0600`, then `mv` atomically.

Use the exact key allowlist from Task 2. Never `source` or `eval` the env file;
parse only known `KEY=value` lines. Never print a secret value. Runtime
commands reject the example file even if explicitly supplied.

Generate all 14 values directly rather than copying the example file. Fixed
non-secret defaults are `komodo-p1`, `komodo_p1_local`, `p1-admin`,
`p1-local`, `/var/run/docker.sock`, and ports 27017/9120/9121/8120.
`P1_PERIPHERY_ROOT` defaults to `$P1_STATE_DIR/periphery`, never `/tmp`.
Only socket, root, and port shell overrides may be consumed while creating a
new file; passwords and JWT/webhook values are always freshly random. Reject
missing, duplicate, or unknown keys when loading an existing runtime file.
Use Python `lstat` to require that an existing env is a non-symlink regular
file owned by the current UID with mode exactly 0600.

Capture `P1_STATE_DIR`, `P1_ENV_FILE`, and `P1_DOCKER_CONTEXT` before parsing,
then keep parsed Compose values in internal shell variables and unset every
ambient Compose interpolation key plus `COMPOSE_FILE`, `COMPOSE_PROJECT_NAME`,
`COMPOSE_ENV_FILES`, `COMPOSE_PROFILES`, and `BUILDX_BUILDER`. The selected env
file is authoritative; shell values for socket/root/ports are consumed only
when creating a new runtime env. This prevents an exported password, Compose
control value, or ambient remote builder from silently changing a later run.

- [ ] **Step 3: Centralize Docker and Compose invocation**

Implement `docker_cmd` and `compose_cmd` so no command can bypass the selected
context, fixed project, explicit env file, or fixed Compose path.

Inspect the context endpoint with:

```sh
docker context inspect "$P1_DOCKER_CONTEXT" \
  --format '{{ (index .Endpoints "docker").Host }}'
```

When `P1_DOCKER_CONTEXT` is unset, resolve it with `docker context show` before
inspection. Always pass the resolved name explicitly to `docker --context`.
Accept only an absolute `unix://` client endpoint. Do not require it to equal
`P1_DOCKER_SOCKET`; the client endpoint and socket mounted into Periphery are
different concepts on OrbStack and Docker Desktop.

- [ ] **Step 4: Implement doctor, config, build, status, down, and reset**

`doctor` checks Docker/Compose/Buildx, daemon reachability, unique valid ports,
port bindability, absolute socket/root paths, writable state, and the required
host tools `curl`, `jq`, `openssl`, Python 3, and `df`. Report free disk and warn
below 30 GiB. An occupied port is acceptable only when inspection proves the
matching running `komodo-p1-local` service owns that exact published binding.
A disposable pinned-Mongo probe must prove the Periphery root and socket are
visible inside the selected local daemon before reporting success. Pulling
that pinned image is allowed; deletion is not.

Inspect the context-named Buildx builder and require its node endpoint to be
the selected local context before any BuildKit or AIO build. Every explicit
Buildx command passes `--builder "$P1_DOCKER_CONTEXT"` after clearing
`BUILDX_BUILDER`.

Use long `--mount type=bind` syntax for both daemon-side probes so a missing
source fails instead of being created by `-v`. Require `test -S` for the socket
and an exact-name, trap-cleaned read/write roundtrip between the host and the
mounted Periphery root.

`doctor` may create exact temporary probe paths, but traps remove them and it
never leaves a state directory or secret env behind. Run every read-only
tool/context/daemon/path/port check before any persistent state creation.

`config` runs `config --quiet`, then prints only service and profile names. It
must render both approved profiles for validation and never print a rendered
environment. `build` explicitly builds Core A and Periphery without starting
services. `build` and `up` run `doctor` first, then call `ensure_runtime_env`
and create external state/Periphery directories with mode 0700 only after the
preflight succeeds. `up` explicitly selects only `mongo`, `core-a`, and
`periphery`, uses `--build`, and then calls `wait`; it must build the current
checkout and must not depend on ambient profile selection. Refuse `up` before
mutation if a Core B container already exists, because rebuilding/restarting
the base beneath a temporary second Core is outside the lab contract.

`wait`, `cross-core-up`, `cross-core-down`, `status`, `down`, and `reset --yes`
require an existing runtime env and never create one. `config` is the only
command allowed to use the example env. Argument errors and every failed
read-only preflight occur before `ensure_runtime_env`.

`down` uses all profiles and `down --remove-orphans`, without volumes.
`reset --yes` uses all profiles and `down --volumes --remove-orphans`, but never
deletes the external state directory. Teardown commands must remain usable when
ports are occupied, bind paths are missing, or services are unhealthy.
`status` uses both profiles with `ps --all` and prints only Compose state,
health, and loopback port data.

- [ ] **Step 5: Implement bounded public and authenticated readiness**

First require Mongo's authenticated healthcheck and repeat an authenticated
`mongosh` ping through
`--profile tools run --rm --no-deps toolbox`. Expand the Mongo credentials only
inside the toolbox container, pass `--authenticationDatabase admin`, and select
the lab database explicitly with
`const lab = db.getSiblingDB(process.env.P1_DATABASE_NAME)`, then require
`lab.runCommand({ ping: 1 }).ok == 1`. Then poll with per-attempt and overall
deadlines:

```text
GET http://127.0.0.1:${P1_CORE_A_PORT}/version
GET http://127.0.0.1:${P1_PERIPHERY_PORT}/version
```

Use a 2-second connect timeout, 5-second per-request limit, 2-second retry
interval, and one 120-second end-to-end budget across all phases of `wait`.
Core B readiness uses its own 120-second budget. The runtime verifier uses a
150-second outer watchdog for `wait` and a 270-second watchdog for
`cross-core-up`.

Then login using the variant endpoint:

```http
POST /auth/login/LoginLocalUser
Content-Type: application/json

{"username":"<local admin>","password":"<local password>"}
```

Require response `.type == "Jwt"` and extract `.data.jwt`. Send the password
body through stdin and the authorization header through `curl --config -` so
neither secret appears in argv.

Prove the real Core-to-Periphery path:

```http
POST /read/GetSystemStats
Authorization: Bearer <jwt>
Content-Type: application/json

{"server":"p1-local"}
```

Require numeric `cpu_perc`, positive `mem_total_gb`, present `polling_rate`, and
positive `refresh_ts`. On timeout, print at most 200 recent lines for Mongo,
Core A, and Periphery after redacting every known secret.

Then POST `/read/ListDockerContainers` with the same JWT and
`{"server":"p1-local"}`. Obtain the full IDs for Mongo, Core A, and Periphery
from the selected context using the fixed Compose project, and require all
three exact IDs in the API response. Merely finding a Unix socket is
insufficient: this proves the mounted socket is a working Docker API for the
same daemon the wrapper controls. Keep the container list in a temporary file;
do not print the full response.

`up` runs the read-only doctor preflight first, then creates state/env, starts
Mongo/Core A/Periphery, and calls `wait`. `wait` never starts services; every
toolbox call includes `--no-deps`.

- [ ] **Step 6: Implement race-closed cross-Core startup**

`cross_core_preflight` runs through
`--profile tools run --rm --no-deps toolbox`, authenticates against `admin`,
selects `db.getSiblingDB(process.env.P1_DATABASE_NAME)`, and requires zero
counts for:

```javascript
const lab = db.getSiblingDB(process.env.P1_DATABASE_NAME);
const counts = {
  in_progress_updates: lab.getCollection("Update").countDocuments({
    status: "InProgress"
  }),
  procedure_schedules: lab.getCollection("Procedure").countDocuments({
    "config.schedule_enabled": { $ne: false },
    "config.schedule": { $type: "string", $ne: "" }
  }),
  action_schedules: lab.getCollection("Action").countDocuments({
    "config.schedule_enabled": { $ne: false },
    "config.schedule": { $type: "string", $ne: "" }
  }),
  startup_actions: lab.getCollection("Action").countDocuments({
    "config.run_at_startup": true
  }),
  gitops_stacks: lab.getCollection("Stack").countDocuments({
    "config.auto_deploy_git_updates": true,
    "config.files_on_host": { $ne: true },
    $and: [
      { $or: [
        { "config.repo": { $type: "string", $ne: "" } },
        { "config.linked_repo": { $type: "string", $ne: "" } }
      ] },
      { $or: [
        { "config.commit": "" },
        { "config.commit": { $exists: false } }
      ] }
    ]
  }),
  gitops_resource_syncs: lab.getCollection("ResourceSync").countDocuments({
    "config.auto_apply_updates": true,
    "config.files_on_host": { $ne: true },
    $and: [
      { $or: [
        { "config.repo": { $type: "string", $ne: "" } },
        { "config.linked_repo": { $type: "string", $ne: "" } }
      ] },
      { $or: [
        { "config.commit": "" },
        { "config.commit": { $exists: false } }
      ] }
    ]
  })
};
print(JSON.stringify(counts));
quit(Object.values(counts).every((count) => count === 0) ? 0 : 42);
```

The wrapper accepts only exit 0 and parses the JSON to require all six named
fields equal zero. Exit 42 is the expected unsafe-state refusal; any other exit
is an infrastructure/query failure, never a false-safe result.

`$ne: false` intentionally treats legacy documents with no
`schedule_enabled` field as enabled.
The GitOps filters mirror the current controller's opt-in predicates. They are
mandatory even with a one-day resource poll interval because Tokio's first
controller tick runs immediately when a second Core starts.

The exact sequence is:

1. Refuse before any preflight if a Core B container already exists.
2. Complete base `wait`.
3. Run the first preflight.
4. Install EXIT/HUP/INT/TERM cleanup traps.
5. Pause Core A.
6. Run the second preflight.
7. Mark Core B as attempted before the start call.
8. Run `--profile cross-core up -d --no-deps core-b`.
9. Login through Core B and repeat `GetSystemStats`.
10. Unpause Core A.
11. Remove traps only after successful unpause.

Cleanup begins with `set +e`, always attempts Core A unpause, and removes a
partially created Core B. `cross-core-down` uses `rm --stop --force core-b` and
does not touch the base services or volumes.

- [ ] **Step 7: Run GREEN wrapper tests**

```sh
rtk sh -n scripts/performance/p1-local.sh
rtk sh -n scripts/performance/p1-local-wrapper.test.sh
rtk sh scripts/performance/p1-local-wrapper.test.sh
rtk sh scripts/performance/check-p1-local-lab.sh
rtk git diff --check
```

Expected:

```text
P1 local wrapper tests OK
P1 local lab contract OK
```

- [ ] **Step 8: Commit checkpoint 2**

```sh
rtk git add \
  scripts/performance/p1-local.sh \
  scripts/performance/p1-local-wrapper.test.sh \
  scripts/performance/check-p1-local-lab.sh
rtk git commit -m "feat: add P1 local lab lifecycle"
```

---

### Task 5: Prove BuildKit isolation and the real runtime lifecycle

**Files:**

- Create: `scripts/performance/check-p1-local-build-context.sh`
- Create: `scripts/performance/verify-p1-local-runtime.sh`
- Output: `target/p1-local-lab/runtime-proof.json`
- Failure output: `target/p1-local-lab/failure-logs/`

- [ ] **Step 1: Create a hermetic BuildKit ignore probe**

For each AIO ignore file, create a temporary build context containing allowed
sentinels plus denied sentinels inside paths that the allowlist would otherwise
re-include:

```text
Cargo.toml
lib/p1-safe
.dev/p1-secret
.git/p1-secret
.worktrees/p1-secret
target/p1-secret
lib/nested/target/p1-secret
lib/nested/node_modules/p1-secret
lib/nested/dist/p1-secret
lib/nested/.env
lib/nested/.envrc
lib/nested/.npmrc
lib/nested/.yarnrc.yml
ui/.env.development
ui/.npmrc
```

Copy the owning ignore file next to a temporary Dockerfile with the matching
`<Dockerfile>.dockerignore` name. The Dockerfile is:

```dockerfile
FROM scratch
COPY . /context
```

Invoke the probe with the exact Dockerfile so BuildKit discovers its sibling
ignore file:

```sh
docker --context "$P1_DOCKER_CONTEXT" buildx build \
  --builder "$P1_DOCKER_CONTEXT" \
  --file "$context/probe.Dockerfile" \
  --output "type=local,dest=$output" \
  "$context"
```

Use Buildx local output, not a pushed or loaded image. A root `safe.txt` is not
a valid positive control because deny-all intentionally excludes it; require
`Cargo.toml` and `lib/p1-safe` in the output and every secret sentinel absent.
The nested `lib` paths prove terminal denies still win after an allowlist
negation, and the UI sentinel covers Core's broad `!ui/**` rule. This proves
actual BuildKit context filtering rather than only checking ignore-file text.

Run the probe after `doctor` and before the expensive AIO builds.
After creating it with `apply_patch`, run
`rtk chmod +x scripts/performance/check-p1-local-build-context.sh`.

- [ ] **Step 2: Write the runtime verifier schema before orchestration**

`verify-p1-local-runtime.sh` uses `set -eu`, owns a temporary workspace, and
atomically writes through `<artifact>.tmp` plus `mv`. Default artifact:

```text
target/p1-local-lab/runtime-proof.json
```

After creating it with `apply_patch`, run
`rtk chmod +x scripts/performance/verify-p1-local-runtime.sh`.

Canonicalize the artifact and temporary paths without following a final
symlink. If the artifact is inside the checkout, require it to remain beneath
`target/p1-local-lab/` and confirm `git check-ignore` accepts it; otherwise
require an absolute path outside the checkout. Refuse tracked, unignored, or
symlink destinations so generated evidence cannot drift into Git.

At entry, remove only a stale artifact/temp artifact and refuse unless there
are zero containers, networks, and volumes labeled
`com.docker.compose.project=komodo-p1-local`. Print the exact explicit recovery
command `p1-local.sh reset --yes`; never reset a pre-existing lab implicitly.
Document that a successful verifier deliberately exercises and resets only the
lab project.

Install a verifier-wide EXIT/HUP/INT/TERM trap before the first failure
injection. Track the unique marker IDs, Core A pause state, Periphery stopped
state, attempted Core B, and outside-canary name. On every exit, use `set +e`
to remove only those markers/canary, unpause Core A, restart Periphery if the
verifier stopped it, and remove an attempted Core B. Preserve the base lab and
redacted failure logs for diagnosis; a subsequent run must still require the
explicit reset precondition.

The verifier is a test runner, not an operator orchestration interface. It must
use the wrapper for normal build/up/wait/down/reset and cross-Core lifecycle.
Its scoped Docker inspections, exact-service stop/start failure injection, and
one-shot toolbox calls must repeat the selected context plus fixed
project/env/file prefix and may never address unrelated objects.

The JSON must include:

```json
{
  "git_sha": "...",
  "git_dirty": false,
  "captured_at_utc": "...",
  "compose_project": "komodo-p1-local",
  "docker_context": "...",
  "docker_endpoint_kind": "unix",
  "host_architecture": "...",
  "engine_architecture": "...",
  "images": {
    "mongo": {"id": "...", "version": "8.0.26", "digest": "sha256:..."},
    "core_a": {"id": "...", "version": "..."},
    "core_b": {"id": "...", "version": "..."},
    "periphery": {"id": "...", "version": "..."}
  },
  "readiness": {
    "core_version": "...",
    "periphery_version": "..."
  },
  "security": {
    "periphery_docker_api_same_daemon": true
  },
  "cross_core": {},
  "lifecycle": {}
}
```

Never store a Mongo URI, JWT, username/password, raw environment, or unredacted
command log. On failure, save only logs after replacing each known secret with
`<redacted>`.

- [ ] **Step 3: Exercise base readiness and source provenance**

The verifier runs:

```text
doctor -> BuildKit ignore probe -> build -> up -> wait
```

Wrap every wrapper call in a verifier-side outer deadline so a regression
cannot hang the proof process. Freeze 7,200 seconds for `build` and `up`, 300
seconds for `doctor`, teardown, and inspection commands, 150 seconds for
`wait`, and 270 seconds for `cross-core-up`. Do not apply the readiness budget
to a cold AIO build.

Record the exact source SHA, dirty flag, selected context and endpoint kind,
host/engine architectures, Mongo digest/version, Core/Periphery image IDs and
versions, Compose service health, and embedded UI HTTP 200.

Require Core A and future Core B to use the same source-built Core image ID.
Inspect mounts and published ports to prove all ports use 127.0.0.1 and only
Periphery has the Docker socket. Repeat the authenticated
`ListDockerContainers` ID comparison and record
`security.periphery_docker_api_same_daemon`. Toolbox must not be a long-running
container.

- [ ] **Step 4: Prove bounded readiness failure and recovery**

Stop Periphery, run `wait`, require bounded failure and a redacted Periphery log
tail, restart Periphery, then require `wait` to recover. A hanging wait is a
test failure.

- [ ] **Step 5: Prove every Core B refusal**

Use the authenticated Core A Write API to create uniquely named, fully
deserializable no-op Procedure and Action resources for the schedule and
startup cases. Use a far-future six-field cron, empty stages/file contents, and
delete each resource through its matching Write API under a per-case cleanup
trap. For the Update case, first create a dedicated unscheduled no-op Procedure
through `/write/CreateProcedure`, run it through `/execute/RunProcedure`, and
wait for its returned Update to become `Complete`. Clone that exact valid BSON
row with a new `_id`, `status: "InProgress"`, `end_ts: null`, fresh timestamp,
and run-specific `other_data` marker, then delete only the clone and the no-op
Procedure. Invoke `cross-core-up`, require refusal before Core B is created, and
clean the case before continuing:

1. `Update.status == "InProgress"`;
2. Procedure with enabled nonempty schedule;
3. Action with enabled nonempty schedule;
4. Action with `config.run_at_startup == true`;
5. Stack opted into pull-based GitOps;
6. Resource Sync opted into pull-based GitOps.

For the GitOps cases, create safe baseline Stack/Resource Sync resources with
no server or Git source through the Write API, clone their fully valid BSON
with new IDs/names, then set only the exact controller opt-in fields (`repo`,
empty `commit`, `files_on_host: false`, and the relevant auto flag). Delete the
exact clones and API baselines in the case trap. Never call the update API with
an enabled source, which could refresh or fetch it through Core A.

Do not insert incomplete Procedure or Action BSON: Core refreshes its
all-resources cache every 15 seconds independently of resource polling. Use an
unmistakable run-specific `_p1_local_harness` marker and delete only resources
or rows created by that run.

- [ ] **Step 6: Prove safe cross-Core success and independent removal**

After all blockers are removed, run `cross-core-up`. Require authenticated
`GetSystemStats` through Core B, identical Core image IDs, and Core A unpaused.
Run `cross-core-down`; require Core B absent while Core A, Periphery, and Mongo
remain healthy.

- [ ] **Step 7: Prove persistence and reset isolation**

Insert a marker in `P1HarnessProof`, run ordinary `down`, run `up`, and require
the marker still present. Create one outside-canary Docker volume without the
Compose project label. Require `reset` without `--yes` to make no changes, then
run `reset --yes` and require no container, volume, or network with label
`com.docker.compose.project=komodo-p1-local` remains. The outside canary and
external state/env directory must remain.

Register an exact-name verifier cleanup trap as soon as the outside canary is
created. Remove that canary explicitly after the assertion and from the trap
on failure; do not include canary cleanup in the wrapper or use a broad volume
filter.

- [ ] **Step 8: Scan all output for secrets and validate the artifact**

Capture wrapper/verifier stdout and stderr in the temporary workspace. Compare
against every secret value read from the generated allowlist. Any match fails
before artifact publication.

Before the atomic `mv`, scan the candidate JSON against every actual secret
value as well. Use `jq -e` to reject forbidden artifact keys such as
`password`, `username`, `jwt`, `credentials`, `mongo_uri`, `database_uri`,
`environment`, `env_file`, `stdout`, `stderr`, and `command_log`, and reject any
string beginning with `mongodb://` or `mongodb+srv://`. Then require every
readiness, security, cross-Core, and lifecycle boolean true, including all six
refusal cases and Core A unpause proof.

- [ ] **Step 9: Run the real runtime gate**

Start OrbStack interactively before this step. Then run:

```sh
rtk env \
  P1_DOCKER_CONTEXT=orbstack \
  P1_DOCKER_SOCKET=/var/run/docker.sock \
  scripts/performance/p1-local.sh doctor

rtk env \
  P1_DOCKER_CONTEXT=orbstack \
  P1_DOCKER_SOCKET=/var/run/docker.sock \
  P1_RUNTIME_ARTIFACT=target/p1-local-lab/runtime-proof.json \
  scripts/performance/verify-p1-local-runtime.sh
```

Expected: exit 0 and one valid proof artifact. Do not relabel a daemon-free or
partial run as runtime evidence.

- [ ] **Step 10: Commit checkpoint 3 test code**

```sh
rtk git add \
  scripts/performance/check-p1-local-build-context.sh \
  scripts/performance/verify-p1-local-runtime.sh
rtk git commit -m "test: verify P1 local lab runtime"
```

The `target/` artifact remains untracked.

---

### Task 6: Document operation, boundaries, rollback, and P1 handoff

**Files:**

- Create: `docs/performance/p1-local-lab.md`
- Modify: `docs/superpowers/specs/2026-07-10-komodo-p1-local-harness-design.md`
- Verify: all four P1 plans

- [ ] **Step 1: Write the operator guide**

The directory does not exist in the current checkout. Create it with
`rtk mkdir -p docs/performance`, then add the guide with `apply_patch`.

Document:

- topology and ports 27017, 8120, 9120, and 9121;
- the exact OrbStack example with context `orbstack` and socket
  `/var/run/docker.sock`;
- all wrapper commands and which ones mutate state;
- default external state path, env mode 0600, and how the operator reads the
  generated local admin password without printing it in logs;
- ordinary `down` persistence and exact `reset --yes` scope;
- Core B's temporary, paused-preflight behavior and non-HA warning;
- artifact location and the fact that it contains no secrets;
- the verifier's empty-project precondition and the fact that a successful run
  deliberately calls `reset --yes` for the fixed lab project;
- recovery from failed `wait` and failed cross-Core startup;
- full rollback command before reverting the implementation.

Include this handoff table:

| Plan | Lab provides | Still owned elsewhere |
|---|---|---|
| Core data | Mongo, toolbox, Core A/B | `p1_*` fixture databases, 1/100/1,000 seeder, production index inventory, staging timing |
| Runtime | Core/Periphery behavior and bounded local failure injection | `komodo_runtime_backpressure_*` fixture and Linux cgroup-v2 4 CPU / 8 GiB gate |
| UI | Embedded UI and local Core target | Vite/HAR/fault proxy checkpoint, CDN and QA evidence |
| Build/release | Source images and application smoke | Actions, GHCR, hosted-runner, cache, and publication evidence |

State explicitly that the lab is a foundation rather than proof that every P1
fixture already exists.

- [ ] **Step 2: Finalize the approved design refinements**

The spec must say `Approved on 2026-07-10` and include:

- exact Mongo digest;
- external state and AIO build-context exclusion;
- local Unix-context restriction;
- browser-reachable distinct Core hosts;
- inbound HTTP Periphery mode;
- selected-daemon Docker API proof through Periphery;
- run-at-startup and pull-GitOps rejection;
- pause/double-preflight/unpause sequence;
- separate test-runner verifier, its scoped Docker exception, proof path, and
  final clean-`HEAD` provenance requirement;
- plan-specific database/seeder handoff.

Scan for `TBD`, `TODO`, `FIXME`, obsolete `proof` wrapper references, internal
`http://core-a`/`core-b` public hosts, or claims that the base lab supplies all
fixtures.

- [ ] **Step 3: Repeat daemon-free gates**

```sh
rtk sh -n scripts/performance/check-p1-local-lab.sh
rtk sh -n scripts/performance/p1-local-wrapper.test.sh
rtk sh -n scripts/performance/p1-local.sh
rtk sh -n scripts/performance/check-p1-local-build-context.sh
rtk sh -n scripts/performance/verify-p1-local-runtime.sh
rtk sh scripts/performance/check-p1-local-lab.sh
rtk sh scripts/performance/p1-local-wrapper.test.sh
rtk env P1_ENV_FILE=compose/p1.local.env.example \
  scripts/performance/p1-local.sh config
rtk docker compose \
  --project-name komodo-p1-local \
  --env-file compose/p1.local.env.example \
  --profile cross-core \
  --profile tools \
  --file p1.local.compose.yaml \
  config --quiet
rtk sh scripts/check-release-targets.sh
rtk git diff --check
```

All commands must exit 0. The contract and wrapper tests must report their
explicit `OK` lines. `config` must not expose example or runtime secrets.

- [ ] **Step 4: Repeat the real runtime proof after all edits**

With OrbStack running:

```sh
rtk env \
  P1_DOCKER_CONTEXT=orbstack \
  P1_DOCKER_SOCKET=/var/run/docker.sock \
  P1_RUNTIME_ARTIFACT=target/p1-local-lab/runtime-proof.json \
  scripts/performance/verify-p1-local-runtime.sh

rtk jq -e '
  .compose_project == "komodo-p1-local" and
  .readiness.authenticated_system_stats == true and
  .readiness.embedded_ui_http_status == 200 and
  .security.all_ports_loopback_only == true and
  .security.docker_socket_only_in_periphery == true and
  .security.periphery_docker_api_same_daemon == true and
  .security.runtime_state_outside_checkout == true and
  .security.secret_output_scan_passed == true and
  .cross_core.absent_by_default == true and
  .cross_core.in_progress_update_refused == true and
  .cross_core.procedure_schedule_refused == true and
  .cross_core.action_schedule_refused == true and
  .cross_core.run_at_startup_refused == true and
  .cross_core.gitops_stack_refused == true and
  .cross_core.gitops_resource_sync_refused == true and
  .cross_core.authenticated_system_stats == true and
  .cross_core.core_a_unpaused == true and
  .cross_core.removed_independently == true and
  .lifecycle.down_preserved_mongo_sentinel == true and
  .lifecycle.reset_without_yes_refused == true and
  .lifecycle.reset_removed_project_resources == true and
  .lifecycle.outside_canary_preserved == true
' target/p1-local-lab/runtime-proof.json
```

- [ ] **Step 5: Review the final diff and commit documentation**

```sh
rtk git status --short --untracked-files=all
rtk git diff --check
rtk git diff --stat origin/main
rtk git diff --name-only origin/main
```

Expected tracked implementation paths are only the approved spec/plan, the
four Compose/build-context files, four performance scripts plus their wrapper
test, and the operator guide. `target/` evidence must remain ignored.

```sh
rtk git add \
  docs/performance/p1-local-lab.md \
  docs/superpowers/specs/2026-07-10-komodo-p1-local-harness-design.md
rtk git commit -m "docs: document P1 local performance lab"
```

- [ ] **Step 6: Produce proof for the final clean commit**

The planning spec/plan must already be committed before Task 1, and Step 5
must leave the final implementation commit clean. Verify that baseline, then
rerun the full verifier so provenance describes final `HEAD`, not the dirty
pre-commit tree:

```sh
rtk git status --short --untracked-files=all

rtk env \
  P1_DOCKER_CONTEXT=orbstack \
  P1_DOCKER_SOCKET=/var/run/docker.sock \
  P1_RUNTIME_ARTIFACT=target/p1-local-lab/runtime-proof.json \
  scripts/performance/verify-p1-local-runtime.sh

rtk jq -e \
  --arg sha "$(rtk git rev-parse HEAD)" \
  --arg digest "sha256:ffa440e8d62533e24a67696ae1bbb46e610ebb3167d65abd122b496ae06d28e6" '
  def nonempty: type == "string" and length > 0;
  .git_sha == $sha and
  .git_dirty == false and
  (.captured_at_utc | nonempty) and
  .compose_project == "komodo-p1-local" and
  .docker_context == "orbstack" and
  .docker_endpoint_kind == "unix" and
  (.host_architecture | nonempty) and
  (.engine_architecture | nonempty) and
  .images.mongo.digest == $digest and
  .images.mongo.version == "8.0.26" and
  (.images.mongo.id | nonempty) and
  (.images.core_a.id | nonempty) and
  (.images.core_b.id | nonempty) and
  .images.core_a.id == .images.core_b.id and
  (.images.periphery.id | nonempty) and
  (.images.core_a.version | nonempty) and
  (.images.core_b.version | nonempty) and
  (.images.periphery.version | nonempty) and
  (.readiness.core_version | nonempty) and
  (.readiness.periphery_version | nonempty) and
  .readiness.authenticated_system_stats == true and
  .readiness.embedded_ui_http_status == 200 and
  .security.all_ports_loopback_only == true and
  .security.docker_socket_only_in_periphery == true and
  .security.periphery_docker_api_same_daemon == true and
  .security.runtime_state_outside_checkout == true and
  .security.secret_output_scan_passed == true and
  .cross_core.absent_by_default == true and
  .cross_core.in_progress_update_refused == true and
  .cross_core.procedure_schedule_refused == true and
  .cross_core.action_schedule_refused == true and
  .cross_core.run_at_startup_refused == true and
  .cross_core.gitops_stack_refused == true and
  .cross_core.gitops_resource_sync_refused == true and
  .cross_core.authenticated_system_stats == true and
  .cross_core.core_a_unpaused == true and
  .cross_core.removed_independently == true and
  .lifecycle.down_preserved_mongo_sentinel == true and
  .lifecycle.reset_without_yes_refused == true and
  .lifecycle.reset_removed_project_resources == true and
  .lifecycle.outside_canary_preserved == true and
  ([.. | objects | keys[]] | any(
    . == "password" or . == "username" or . == "jwt" or
    . == "credentials" or . == "mongo_uri" or
    . == "database_uri" or . == "environment" or
    . == "env_file" or . == "stdout" or . == "stderr" or
    . == "command_log"
  ) | not) and
  ([.. | strings] | any(test("^mongodb(\\+srv)?://")) | not)
' target/p1-local-lab/runtime-proof.json

rtk git status --short --untracked-files=all
rtk proxy git show --check --oneline HEAD
```

Both status commands must be empty. The ignored proof describes exact clean
`HEAD`; the successful verifier ends with only lab-owned Compose resources
reset and leaves the external env/state directory intact.

---

## Final review checklist

- [ ] Static contract was observed RED before Compose files existed and GREEN
  after implementation.
- [ ] Wrapper black-box tests were observed RED before the wrapper and GREEN
  after implementation.
- [ ] Mongo and toolbox use the exact 8.0.26 multi-arch digest.
- [ ] AIO Docker contexts exclude all ignored secret and build-output paths,
  with a real BuildKit local-output probe.
- [ ] Runtime state and secrets live outside the checkout and remain mode 0600.
- [ ] Only a local Unix-socket Docker context is accepted.
- [ ] Every Compose invocation uses the fixed project, explicit env, and fixed
  file path.
- [ ] Config output never renders runtime environment values.
- [ ] Core A and B have browser-reachable distinct hostnames and Mongo app
  names.
- [ ] Periphery is inbound HTTP with public-key authentication and has the only
  Docker socket mount.
- [ ] Authenticated container listing proves that socket reaches the same
  selected daemon and returns the lab's exact container IDs.
- [ ] `wait` proves authenticated Core-to-Periphery stats, not only public
  liveness.
- [ ] Core B is absent by default and unsafe startup is rejected for
  InProgress, Procedure schedule, Action schedule, startup Action, GitOps
  Stack, and GitOps Resource Sync cases.
- [ ] Core A is paused across the second preflight and Core B start, and every
  error path unpauses it.
- [ ] Ordinary down preserves Mongo data; reset requires `--yes` and removes
  only project-labeled state.
- [ ] Runtime proof is atomic, secret-free, current, and stored only in
  `target/`.
- [ ] The guide clearly separates local proof from plan-specific fixtures,
  Linux staging, QA/CDN, Actions, GHCR, and production acceptance.
- [ ] No unrelated Compose helper, production code, fixture seeder, browser
  harness, or release workflow changed.
- [ ] Branch and any future PR remain scoped to `intezya/komodo:main`.

## Execution handoff

Execute Tasks 1–6 in order. Use one fresh implementation agent per task, then a
spec-compliance review followed by a code-quality review. Do not parallelize
implementation agents because Tasks 1–4 evolve the same contract and wrapper.

Tasks 1–4 and all daemon-free validation can begin while OrbStack is stopped.
Task 5 blocks on starting OrbStack and must not be marked complete from mocks or
static Compose rendering. Task 6 repeats both static and runtime evidence after
the final documentation/spec edits.
