# P1 local performance lab

This lab is the local integration foundation for the Komodo P1 performance
plans. It builds the current checkout and runs a fixed Compose project named
`komodo-p1-local`. It does not replace the fixture, Linux staging, browser,
Actions, GHCR, CDN, QA, or production gates owned by those plans.

## Topology

| Service | Host endpoint | Purpose |
|---|---|---|
| MongoDB 8.0.26 | `127.0.0.1:27017` | Persistent local lab database |
| Periphery | `127.0.0.1:8120` | Inbound HTTP and selected Docker socket |
| Core A | `127.0.0.1:9120` | Primary API and embedded UI |
| Core B | `127.0.0.1:9121` | Temporary cross-Core test participant |

Core B is not an HA replica. It uses the same database and Periphery as Core A
only during a controlled test. All published ports are loopback-only.

## OrbStack quick start

Run commands from the repository root:

```sh
export P1_DOCKER_CONTEXT=orbstack
export P1_DOCKER_SOCKET=/var/run/docker.sock

rtk scripts/performance/p1-local.sh doctor
rtk scripts/performance/p1-local.sh build
rtk scripts/performance/p1-local.sh up
rtk scripts/performance/p1-local.sh status
```

Open `http://127.0.0.1:9120` for the embedded UI. Runtime configuration lives
outside the checkout at
`${P1_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/komodo-p1-local}`.
The generated `p1-local.env` is mode `0600`. To load the local admin password
without printing it or putting it in shell history, read it into a variable and
unset it after use:

```sh
env_file="${P1_ENV_FILE:-${P1_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/komodo-p1-local}/p1-local.env}"
P1_ADMIN_PASSWORD=$(awk -F= '$1 == "P1_INIT_ADMIN_PASSWORD" { sub(/^[^=]*=/, ""); print; exit }' "$env_file")
# Use "$P1_ADMIN_PASSWORD" without echoing it.
unset P1_ADMIN_PASSWORD
```

Never use production or QA credentials with this lab.

## Commands and state changes

| Command | Behavior |
|---|---|
| `doctor` | Read-only preflight apart from temporary, removed filesystem probes |
| `config` | Renders the fixed Compose model; starts no services |
| `build` | Creates the external env if absent and source-builds Core and Periphery images |
| `up` | Starts Mongo and Core A, exports Core A's public key, starts Periphery, then waits for authenticated readiness |
| `wait` | Reads health/API state only; fails within a bounded deadline |
| `cross-core-up` | Double-checks safety, pauses Core A across the second check and Core B start, verifies Core B, and always unpauses Core A |
| `cross-core-down` | Removes only Core B and waits for its host port to be released |
| `status` | Reads Compose service/health state without showing secrets |
| `down` | Removes lab containers and network, preserves named volumes and external state |
| `reset --yes` | Removes only the fixed project's containers, network, and named volumes; preserves external env/state |

`reset` without `--yes` is rejected. Teardown commands wait for lab ports to
be released, which prevents a following `doctor` or `up` from racing the local
engine's port proxy.

## Core B safety

`cross-core-up` refuses to start Core B if any Update is `InProgress`, any
Procedure or Action schedule is enabled, any Action has `run_at_startup`, or a
Stack/Resource Sync is opted into pull GitOps. It checks once while Core A is
running, pauses Core A, checks again directly in MongoDB, starts and verifies
Core B, then unpauses Core A. Its trap unpauses Core A and removes an attempted
Core B on every error path.

If cross-Core startup fails, inspect `status`, run `cross-core-down`, and then
run `wait`. If ordinary `wait` fails, leave the base stack running, inspect
`status` and scoped Compose logs, correct the local engine or service problem,
and rerun `wait`. Use `reset --yes` only when discarding the local database is
intentional.

## Runtime proof

The real verifier requires no existing container, network, or volume labeled
for `komodo-p1-local`; it never resets a pre-existing lab implicitly. Start
from an explicit reset, then run:

```sh
rtk scripts/performance/p1-local.sh reset --yes
rtk env \
  P1_DOCKER_CONTEXT=orbstack \
  P1_DOCKER_SOCKET=/var/run/docker.sock \
  P1_RUNTIME_ARTIFACT=target/p1-local-lab/runtime-proof.json \
  scripts/performance/verify-p1-local-runtime.sh
```

A successful verifier deliberately exercises `down`, persistence, cross-Core
failure and success paths, and finally `reset --yes` for only the fixed lab
project. It writes `target/p1-local-lab/runtime-proof.json` atomically. The
ignored artifact records clean source/runtime provenance and boolean results,
not credentials, JWTs, database URIs, raw environments, or command logs.
External env/state remains in place.

## P1 handoff

| Plan | Lab provides | Still owned elsewhere |
|---|---|---|
| Core data | Mongo, toolbox, Core A/B | `p1_*` fixture databases, 1/100/1,000 seeder, production index inventory, staging timing |
| Runtime | Core/Periphery behavior and bounded local failure injection | `komodo_runtime_backpressure_*` fixture and Linux cgroup-v2 4 CPU / 8 GiB gate |
| UI | Embedded UI and local Core target | Vite/HAR/fault proxy checkpoint, CDN and QA evidence |
| Build/release | Source images and application smoke | Actions, GHCR, hosted-runner, cache, and publication evidence |

The lab is a foundation; its proof does not assert that any of these
plan-specific fixtures or external acceptance gates already exist.

## Rollback

Remove local lab state before reverting the implementation:

```sh
rtk scripts/performance/p1-local.sh reset --yes
```

Confirm that only `komodo-p1-local` resources are gone, then revert the four
local-lab commits. Do not replace the command with a broad Docker prune or a
volume filter that could address unrelated projects.
