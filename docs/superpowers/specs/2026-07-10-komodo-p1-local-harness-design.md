# Komodo P1 Local Performance Lab Design

**Status:** Approved on 2026-07-10

## Context

The four approved P1 performance plans need a repeatable local integration
environment. The repository already provides two useful but incomplete paths:

- `dev.compose.yaml` builds Core and Periphery from the current checkout, but
  uses FerretDB and has only one Core.
- `compose/mongo.compose.yaml` provides MongoDB, Core, and Periphery, but uses
  published GHCR images rather than the current checkout.

Neither path provides the controlled second Core, deterministic readiness,
safe local secrets, or a stable interface for plan-specific fixture and fault
injection tools. The shared lab should provide those foundations without
absorbing implementation work owned by the four P1 plans.

## Goals

- Build and run the current checkout as a local MongoDB-backed stack.
- Start MongoDB, Core A, and Periphery with one command and deterministic
  readiness checks.
- Start Core B only for controlled cross-Core tests.
- Bind every host-facing port to loopback.
- Keep credentials, keys, databases, and generated evidence out of Git.
- Give later P1 checkpoints a stable Compose project, network, database, and
  toolbox interface.
- Make teardown safe by preserving data by default and requiring an explicit
  reset to remove lab-owned volumes.
- Work with a selected local Unix-socket Docker context while documenting
  `orbstack` for this Mac.

## Non-Goals

- Treat local macOS or QEMU timings as production performance evidence.
- Model Core B as an always-on HA replica. It is a temporary test participant.
- Read production databases, publish images, write GHCR caches, or dispatch
  GitHub Actions.
- Implement the resource fixture seeder from Plan 1.
- Implement the UI HAR collector or fault proxy from Plan 3.
- Replace plan-specific regression, load, browser, or release tests.
- Repair the existing `dev-compose-exposed` helper as unrelated cleanup.

## Considered Approaches

### Reuse the published-image Mongo Compose stack

This is the smallest setup, but it validates released images instead of the
branch under test. Overriding only image tags would also couple local work to a
registry publishing step. Rejected for implementation testing.

### Run MongoDB in Docker and Core, Periphery, and UI on the host

This gives the fastest Rust and UI edit loop and remains a supported manual
development path. It requires several long-running terminals, host-installed
tools, PID management, and host-specific cleanup. Keep it documented as the
fast developer loop, but do not use it as the reproducible lab contract.

### Add a dedicated source-built Compose lab

This is the selected approach. It reuses the current all-in-one Core and
Periphery Dockerfiles, isolates state under one Compose project, and gives
scripts and CI a deterministic service topology. Builds are slower than the
hybrid path, but Docker caching makes subsequent runs practical and the lab
actually exercises the current checkout.

## Architecture

The base topology is:

```text
Browser -> Core A embedded UI / API (127.0.0.1:9120)
                         |
                         +-> MongoDB 8 (127.0.0.1:27017)
                         +-> Periphery (127.0.0.1:8120)
                                  |
                                  +-> selected Docker engine socket

Cross-Core profile only:
Browser/test -> Core B API (127.0.0.1:9121) -> same MongoDB and Periphery
```

### Compose definition

Create `p1.local.compose.yaml` at the repository root, alongside the existing
development Compose files. Use the fixed project name `komodo-p1-local` from
the wrapper rather than embedding it in the Compose file.

Services:

- `mongo`: authenticated
  `mongo:8.0.26@sha256:ffa440e8d62533e24a67696ae1bbb46e610ebb3167d65abd122b496ae06d28e6`,
  persistent data and config volumes, loopback-only port, and a `mongosh` ping
  healthcheck. The pinned index contains Linux amd64 and arm64/v8 manifests.
  Updating the digest is an explicit dependency change accompanied by the
  resolved version and architecture list.
- `core-a`: build `bin/core/aio.Dockerfile` from the current checkout, share
  the lab keys volume, use the lab Mongo database, expose port 9120 on
  loopback, and wait for healthy MongoDB.
- `periphery`: build `bin/periphery/aio.Dockerfile`, run explicitly in inbound
  mode with `PERIPHERY_SSL_ENABLED=false` and
  `PERIPHERY_CORE_PUBLIC_KEYS=file:/config/keys/core.pub`, share the keys
  volume, mount the selected local engine socket plus `/proc`, bind one
  absolute daemon-local lab root at the same path inside and outside the
  container, and expose port 8120 only on loopback.
- `core-b`: use the same Core build and database under Compose profile
  `cross-core`, expose host port 9121, disable bootstrap resource creation, and
  start only after Core A is ready.
- `toolbox`: use the MongoDB image under profile `tools` so plan scripts can
  run `mongosh` without a host installation.

Core A and Core B share the database, JWT secret, and Core/Periphery keys. Both
Core services use inbound Periphery at `http://periphery:8120`; no
`PERIPHERY_CORE_ADDRESS` is set. They use distinct internal
`KOMODO_HOST` values, `http://127.0.0.1:${P1_CORE_A_PORT}` and
`http://localhost:${P1_CORE_B_PORT}`, because Periphery keys inbound channels
by Core hostname while both values remain reachable from the host. Core A owns
initial admin and first-server bootstrap, while default Procedures and Actions
are disabled for the lab. Core B must never start while an Update is
`InProgress`, an enabled Procedure or Action schedule exists, or an Action has
`run_at_startup` enabled, or an opt-in GitOps Stack/Resource Sync exists because
each Core runs background loops, startup recovery, startup Actions, and an
immediate first GitOps reconciliation tick. `cross-core-up` pauses Core A,
proves those conditions directly in MongoDB a second time, starts and verifies
Core B, then unpauses Core A. A trap must always unpause Core A and remove a
partially started Core B on failure, closing the preflight-to-start race.

Create `bin/core/aio.Dockerfile.dockerignore` and
`bin/periphery/aio.Dockerfile.dockerignore`. They must exclude at least `.dev`,
`.git`, `.worktrees`, `target`, all dependency output directories, and all
`.env` variants. The contract test must prove that ignored local development
secrets cannot enter either build context.

The base lab serves the production-built embedded UI from Core A. The existing
host Vite workflow remains the faster UI development path. Plan 3 may later add
its loopback fault proxy and browser automation without changing the base
service contract.

### Local configuration and secrets

Commit `compose/p1.local.env.example` with non-secret variable names and safe
loopback defaults. The wrapper stores runtime state outside the checkout under
`P1_STATE_DIR`, defaulting to
`${XDG_STATE_HOME:-$HOME/.local/state}/komodo-p1-local`. It creates
`$P1_STATE_DIR/p1-local.env` with mode `0600` and random MongoDB, JWT, webhook,
and initial-admin secrets when it is absent. Keeping runtime secrets outside
the checkout is mandatory even though `.dev` is Git-ignored.

The wrapper must not print secret values. It may print the environment-file
path and the initial admin username. Users who need the password read the
ignored file locally.

The wrapper always invokes Compose with
`--env-file "${P1_ENV_FILE:-$P1_STATE_DIR/p1-local.env}"`. Static clean-checkout
validation sets `P1_ENV_FILE=compose/p1.local.env.example`; runtime commands
refuse the example file and use the generated state file.

Required user-supplied configuration is limited to:

- `P1_DOCKER_CONTEXT` when the current local context is not desired;
- `P1_DOCKER_SOCKET`, an absolute Unix socket path visible to the daemon host;
- `P1_PERIPHERY_ROOT`, an absolute daemon-local directory, defaulting below
  `P1_STATE_DIR`;
- optional loopback port overrides for collision avoidance.

SSH, TCP, and other remote Docker contexts are rejected because their daemon
cannot safely consume local socket and directory mounts. No production URI or
credential is accepted by the base lab.

## Orchestration Interface

Create `scripts/performance/p1-local.sh` with these commands:

- `doctor`: check that the selected context exists, uses a local Unix socket,
  and has a running Docker daemon; then check Compose, Buildx, available disk,
  the engine socket, the absolute daemon-local Periphery root, and local port
  availability. Report free disk and warn below 30 GiB without deleting
  anything. On this Mac the docs recommend
  `P1_DOCKER_CONTEXT=orbstack`.
- `config`: render and validate the effective Compose model without starting
  services.
- `build`: build Core and Periphery from the current checkout.
- `up`: initialize the ignored environment if needed, start the base services,
  and wait for health. It starts MongoDB and Core A first, reads Core A's
  public key through the authenticated API, writes only that public key into
  the shared keys volume through the one-shot toolbox, and then starts
  Periphery. This avoids relying on host access to a daemon-managed volume.
- `wait`: prove MongoDB, Core A `GET /version`, and Periphery HTTP
  `GET /version` readiness with bounded retries. Then authenticate to Core A
  with the generated local admin and make one Periphery-backed stats request
  through the bootstrapped first Server; component liveness alone is not a
  successful readiness result. It then lists Docker containers through Core A
  and requires the exact Mongo/Core A/Periphery IDs reported by the selected
  context, proving the mounted socket reaches the same working daemon.
- `cross-core-up`: refuse to proceed unless the base stack is healthy and the
  first Mongo preflight is safe. Pause Core A, repeat the Mongo preflight for
  `InProgress` Updates, enabled Procedure/Action schedules, startup Actions,
  and opt-in GitOps Stacks/Resource Syncs, then start Core B, authenticate
  through it, and make the same
  Periphery-backed stats request. Unpause Core A on success and on every error.
- `cross-core-down`: stop and remove only Core B while preserving the base
  stack and all volumes, then wait within a bounded deadline for its loopback
  port to be released.
- `status`: show Compose service and health state without exposing secrets.
- `down`: stop containers and networks while preserving volumes, then wait
  within a bounded deadline for all lab ports to be released.
- `reset --yes`: remove only the `komodo-p1-local` containers, networks, and
  volumes, then wait within a bounded deadline for all lab ports to be
  released. Refuse without the explicit flag.

Users invoke the wrapper through `rtk`, but the checked-in script itself calls
the underlying tools directly, matching existing repository scripts.

The runtime verifier is a test runner rather than an operator orchestration
surface. It uses the wrapper for normal build, readiness, cross-Core, teardown,
and reset behavior, but may perform fixed-project read-only inspection,
exact-service stop/start failure injection, and one-shot toolbox commands. All
such calls must repeat the selected local context, project, env file, and
Compose file and must never address unrelated Docker objects.

## Extension Contracts for the P1 Plans

Plan 1 may add fixture and profiler scripts that use the `toolbox` profile,
the loopback Mongo port, and the generated database credentials. Cardinality
fixtures and production index inventory remain owned by Plan 1. Its checkpoint
must create separate manifest-backed `p1_*` databases for 1/100/1,000; the base
lab neither provisions nor relabels them.

Plan 2 may use the base stack for Core/Periphery behavior and the temporary
Core B profile for event and permission tests. Its normative RSS and scheduler
gates still require the declared Linux cgroup-v2 preflight. Its staging scripts
own a separate `komodo_runtime_backpressure_*` database and must not reuse the
base functional database as measurement evidence.

Plan 3 may point Vite or production preview at Core A, then add the planned
fault proxy and HAR tooling. The base lab does not add Playwright or redefine
the UI plan's browser budgets.

Plan 4 may reuse the built images for smoke tests. GitHub Actions concurrency,
GHA cache behavior, GHCR lifecycle, hosted-runner timings, and release
publication remain remote acceptance gates.

Create `docs/performance/p1-local-lab.md` as the operator entry point. It must
state that the fixture seeders and browser fault/HAR harness arrive in their
own P1 checkpoints rather than implying that the base lab alone closes those
acceptance gates.

Create `scripts/performance/verify-p1-local-runtime.sh` as the machine-readable
runtime verifier. It reruns readiness and lifecycle/failure-path checks, then
atomically writes `target/p1-local-lab/runtime-proof.json` unless
`P1_RUNTIME_ARTIFACT` overrides the path. The artifact contains the source
commit and dirty flag, selected context, host and engine architectures, pinned
Mongo image, built image IDs, component versions, service health, cross-Core
results, and reset-isolation results. It must not contain credentials, database
URIs, or raw environment dumps.

The verifier requires an empty fixed Compose project at entry and never resets
pre-existing lab state implicitly. A successful run deliberately exercises
and resets only `komodo-p1-local`, while preserving the external environment
and state directory. The accepted final proof must be regenerated after the
documentation commit so `git_sha` names clean final `HEAD` and `git_dirty` is
false.

## Error Handling and Safety

- Fail before mutation when Docker is unavailable, the selected context does
  not use a local Unix socket, the socket is unavailable, the Periphery root is
  not absolute on the daemon host, or a host port is occupied.
- Bound every readiness loop and print the failing service's recent logs on
  timeout.
- Never enable Core B by default.
- Never enable Core B when an Update is `InProgress` or an Action or Procedure
  has a nonempty enabled schedule, or when an Action has `run_at_startup`
  enabled, or while a Stack/Resource Sync is opted into pull-based GitOps.
- Never use `down -v` from ordinary teardown.
- Restrict reset to the fixed Compose project and require `--yes`.
- Bind MongoDB, Core, and Periphery ports to `127.0.0.1` only.
- Mount the Docker socket only into Periphery.
- Do not silently fall back from MongoDB to FerretDB or from source builds to
  published images.

## Verification Strategy

Start with a failing static contract test in
`scripts/performance/check-p1-local-lab.sh`. It must reject the repository
before the lab exists and then verify:

- the exact five service names and the two opt-in profiles;
- source build contexts for Core and Periphery;
- MongoDB 8 pinned by digest rather than FerretDB or a tag-only image;
- loopback-only host ports;
- the shared database, network, key volume, and fixed Compose project wrapper;
- distinct Core hostnames and Core B bootstrap/startup restrictions;
- explicit inbound HTTP Periphery mode and authenticated Core-to-Periphery
  readiness through both Core services;
- external runtime state, explicit env-file selection, local Unix-socket
  context validation, and Dockerfile-specific secret exclusions;
- absence of GHCR image references and production-looking hosts;
- explicit safe teardown and reset behavior.

Static gates:

```sh
rtk sh -n scripts/performance/p1-local.sh
rtk sh -n scripts/performance/check-p1-local-lab.sh
rtk sh scripts/performance/check-p1-local-lab.sh
rtk env P1_ENV_FILE=compose/p1.local.env.example \
  scripts/performance/p1-local.sh config
rtk docker compose --env-file compose/p1.local.env.example \
  -f p1.local.compose.yaml config --quiet
rtk git diff --check
```

Runtime gates, after starting the selected Docker engine:

```sh
rtk env P1_DOCKER_CONTEXT=orbstack \
  scripts/performance/p1-local.sh doctor
rtk env P1_DOCKER_CONTEXT=orbstack \
  scripts/performance/p1-local.sh build
rtk env P1_DOCKER_CONTEXT=orbstack \
  scripts/performance/p1-local.sh up
rtk env P1_DOCKER_CONTEXT=orbstack \
  scripts/performance/p1-local.sh cross-core-up
rtk env P1_DOCKER_CONTEXT=orbstack \
  scripts/performance/p1-local.sh status
rtk env P1_DOCKER_CONTEXT=orbstack \
  scripts/performance/p1-local.sh cross-core-down
rtk env P1_DOCKER_CONTEXT=orbstack \
  scripts/performance/p1-local.sh down
rtk env P1_DOCKER_CONTEXT=orbstack \
  P1_RUNTIME_ARTIFACT=target/p1-local-lab/runtime-proof.json \
  scripts/performance/verify-p1-local-runtime.sh
```

The runtime proof records image IDs, source commit, service health, component
versions, selected Docker context, host architecture, and Docker architecture.
It does not claim production performance parity.

## Acceptance Criteria

- A clean checkout can render the lab configuration without local secrets.
- With a running Docker engine, one command builds and starts the base stack
  from the current checkout.
- MongoDB, Core A, and Periphery become healthy within bounded time, an
  authenticated Periphery-backed stats request succeeds through Core A, and an
  authenticated container listing proves Periphery uses the selected daemon.
- The embedded UI is reachable from Core A.
- Core B is absent by default, refuses unsafe startup, and can complete the
  same authenticated Periphery-backed request before being stopped
  independently.
- Ordinary teardown preserves data; explicit reset removes only lab-owned
  state.
- All host ports are loopback-only and no secret is committed or printed.
- Runtime secrets and Periphery state remain outside the Docker build context,
  and both AIO ignore files exclude existing ignored development secrets.
- The final JSON proof artifact is produced from clean final `HEAD`, contains
  the exact source/runtime provenance, and contains no credential or raw
  environment value.
- The documentation distinguishes local regression proof from Linux staging,
  GitHub Actions/GHCR, CDN, and production acceptance.

## Rollback

The lab has no production schema or runtime effect. Revert the Compose file,
wrapper, contract test, example environment, and documentation. If local state
was created, run the version of `reset --yes` from the implementing commit
before reverting, or remove only the `komodo-p1-local` Compose project after
reviewing `docker compose ps` and `docker volume ls`.
