# Native Compose Replicas and Rolling Updates

## Goal

Add native Docker Compose replica awareness to Komodo and an opt-in,
zero-downtime rolling deployment strategy implemented by Periphery. The
feature must not depend on the external `docker-rollout` plugin and must
preserve existing deployment behavior unless enabled on a Stack.

## Scope

This change covers Compose-mode Stacks only. Swarm behavior is unchanged.
Replica counts remain declarative in the Compose files; Komodo will not add a
separate scale control to its API or UI.

The first version uses a fixed maximum surge of one container. Configurable
surge, parallel replacement, automatic reverse rollouts, and per-service
replica editing are out of scope.

## Configuration

Add `rolling_update: bool` to `StackConfig`, defaulting to `false`. With the
default value, Komodo continues to run its existing `docker compose up -d`
flow.

When `rolling_update` is enabled, every selected Compose service uses rolling
deployment unless its resolved Compose labels contain:

```yaml
labels:
  komodo.rollout: "false"
```

An optional pre-stop hook is configured on a service with:

```yaml
labels:
  komodo.rollout.pre-stop-hook: "touch /tmp/drain && sleep 10"
```

The hook runs inside each old container immediately before that container is
stopped. An empty or absent label means no hook.

`destroy_before_deploy` and `rolling_update` are incompatible. Periphery must
reject this combination before changing any containers because destroying the
project cannot provide zero-downtime deployment.

## Resolved Compose Model

Periphery uses the output of `docker compose config` as the deployment source
of truth. The minimal internal Compose model will include:

- service name and image;
- `deploy.replicas`, with an omitted value treated as one;
- `container_name`;
- published ports;
- resolved labels required by Komodo.

The model remains internal and narrowly typed. Flexible Compose fields that
only need presence checks may use a serde value instead of duplicating the
full Compose specification.

Before a rolling deployment, Periphery rejects a selected rolling service if
it declares `container_name` or published host ports. Either setting prevents
old and new containers from running concurrently. The error must identify the
service and tell the operator to remove the incompatible setting or add
`komodo.rollout: "false"`. Komodo does not silently fall back to a deployment
with downtime.

## Deployment Flow

The existing preparation stages retain their order:

1. write or update Stack files;
2. validate required files and the resolved Compose configuration;
3. log in to the registry when configured;
4. execute pre-deploy;
5. build when configured;
6. pull images when configured;
7. deploy selected services;
8. execute post-deploy after complete success.

With `rolling_update: false`, step 7 remains the current Compose up command.

With `rolling_update: true`, Periphery separates the selected services into
rolling and opted-out services. Opted-out services are updated individually
with `docker compose up -d --no-deps <service>` so the command cannot recreate
rolling services. Rolling services are processed sequentially. A deploy with
an empty service filter processes all services from the resolved Compose
configuration; a filtered deploy touches only the requested services.

A service with no running containers is started normally at its desired
replica count. It has no availability to preserve and therefore does not need
the replacement algorithm.

## Per-Replica Rolling Algorithm

For a running rolling service with a desired replica count of `N`, Periphery:

1. discovers the original container IDs using Docker Compose project and
   service labels;
2. verifies the starting state and records the original IDs;
3. runs Compose with `--no-deps --no-recreate --scale <service>=N+1` to create
   one container from the new resolved configuration;
4. discovers the service containers again and requires exactly one previously
   unseen ID;
5. waits for the new container to become ready;
6. runs the optional pre-stop hook in one original container;
7. stops and removes that original container;
8. verifies that exactly `N` service containers remain;
9. repeats steps 3 through 8 until every original ID has been replaced.

After the loop, Periphery verifies that the service has exactly `N` running
containers and none of the original IDs remain. Services and replicas are
updated sequentially so temporary capacity is at most one container above the
declared replica count.

All Compose invocations preserve the Stack project name, file arguments,
environment files, command wrapper behavior, working directory, and secret
replacement. Existing extra deployment arguments are appended to each Compose
up invocation. Before changing containers, rolling validation rejects arguments
that directly conflict with orchestration, including `--force-recreate`,
`--always-recreate-deps`, `--scale`, and an explicit service selection. The
error identifies the conflicting argument instead of silently dropping it.

## Readiness

If the new container has a Docker healthcheck, Periphery waits for `healthy`
for up to 60 seconds. A terminal unhealthy state or timeout fails the rollout.

If the container has no healthcheck, it must remain in the Docker `running`
state for 10 seconds. Exiting or restarting during that period fails the
rollout.

The initial implementation uses these fixed values to match the practical
defaults of `docker-rollout`. User-configurable timing is out of scope.

## Failure Semantics

If a new container does not become ready, Periphery captures its health state
and a bounded tail of its logs, then stops and removes it. The corresponding
old container remains running and the entire Stack deployment stops.

If the pre-stop hook fails, the old container is not removed. Periphery
removes the new container and stops the deployment.

If cleanup of the new container also fails, both errors are reported and
Periphery makes no further destructive changes.

Successfully replaced replicas are not automatically reverted if a later
replica fails. The old image or configuration may no longer be available, so
an attempted reverse rollout could make the service less available. Komodo
reports the operation as a partial deployment with enough stage and container
detail for an operator to retry or intervene.

Post-deploy runs only after every selected service succeeds.

## Logs and Secret Handling

Each important phase emits a distinct Komodo update log: rolling validation,
scale up, readiness, pre-stop hook, old-container removal, cleanup, and final
verification. Commands and captured output use the existing sanitization and
secret-replacement path. Container logs included after a readiness failure are
bounded to avoid oversized updates.

## Native Replica State

`deploy.replicas` is represented as one Compose service with a desired replica
count, rather than artificial service names such as `web-1` and `web-2`.

Runtime monitoring discovers all containers belonging to the service using
the standard `com.docker.compose.project` and `com.docker.compose.service`
labels. The runtime API exposes the collection of matching containers while
retaining the previous single-container field during a compatibility period.

Service state compares actual container readiness with the desired replica
count:

- all desired replicas ready: running;
- some missing, unhealthy, restarting, or extra: degraded;
- no containers: stopped or not deployed according to existing Stack rules.

`ignore_services` continues to match the original Compose service name. The UI
shows an `actual / desired replicas` summary and allows the operator to inspect
each container's status, health, and image. Singleton services keep the
existing compact presentation.

## Compatibility

Existing Stack documents deserialize with `rolling_update: false`, preserving
their current deployment behavior. Generated schemas and TypeScript types are
updated with the new Stack setting and replica-aware runtime fields.

Swarm Stacks, normal Compose lifecycle commands, and deployment procedures that
do not enable rolling updates are not changed. Automatic image and Git update
flows use the same Stack setting when they eventually call the existing deploy
path.

## Verification

Automated coverage includes:

- parsing replica counts, labels, container names, and published ports;
- legacy Stack configuration deserialization with rolling updates disabled;
- command construction with project, file, environment, service, and wrapper
  arguments;
- old/new container set detection;
- healthy, no-healthcheck, timeout, hook failure, cleanup failure, and partial
  rollout state-machine paths;
- incompatibility validation before container changes;
- monitoring multiple containers for one Compose service;
- state calculation for satisfied, missing, unhealthy, and extra replicas;
- the unchanged non-rolling Compose deployment path.

Repository verification includes Rust formatting, targeted Rust tests, the
relevant workspace build, generated-client consistency, and the UI build.

A local Compose integration harness uses a service with two replicas and a
healthcheck. It records readiness throughout a rollout and proves that at
least one ready replica remains available while container IDs are replaced.
Additional cases cover a failed healthcheck rollback, an opted-out singleton,
and rejection of incompatible ports or `container_name`.
