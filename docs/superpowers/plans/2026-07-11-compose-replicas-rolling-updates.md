# Compose Replicas and Rolling Updates Implementation Plan

**Spec:** `docs/superpowers/specs/2026-07-11-compose-replicas-rolling-updates-design.md`

**Goal:** Make Compose replicas first-class in Komodo monitoring and add an
opt-in, Periphery-native rolling deployment with a maximum surge of one.

**Implementation rules:** Work test-first, keep the existing non-rolling path
unchanged, use resolved `docker compose config` as the deployment source of
truth, and commit each task independently. Run every shell command through
`rtk`.

## Task 1: Add backward-compatible Stack configuration

**Files:**

- Modify `client/core/rs/src/entities/stack.rs`
- Modify existing Stack config compatibility tests near the entity or config
  serialization tests
- Regenerate `client/core/ts/src/types.ts`

**Steps:**

1. Add a failing deserialization/default test proving an old Stack document
   produces `rolling_update == false`.
2. Add `rolling_update: bool` to `StackConfig` with serde, builder, partial,
   schema, and documentation attributes matching adjacent boolean settings.
3. Add a serialization round-trip test for `rolling_update: true`.
4. Generate TypeScript types with
   `rtk node client/core/ts/generate_types.mjs`.
5. Run the smallest client/entity test target and
   `rtk cargo fmt --all -- --check`.
6. Commit as `feat: configure compose rolling updates`.

## Task 2: Define the resolved Compose rollout model

**Files:**

- Modify `client/core/rs/src/entities/stack.rs`
- Modify `bin/periphery/src/api/compose.rs`
- Add focused tests beside the Compose parsing helpers

**Steps:**

1. Add failing parsing tests for:
   - omitted replicas defaulting to one;
   - numeric and interpolated-string replica values;
   - map and list label syntax;
   - `komodo.rollout: "false"`;
   - `komodo.rollout.pre-stop-hook`;
   - `container_name` and short/long published port syntax.
2. Extend the minimal internal `ComposeService` model only with the fields
   needed for replica monitoring and rollout validation.
3. Introduce a small parsed rollout-policy type rather than passing raw label
   strings through deployment code.
4. Add validation tests proving incompatible rolling services produce an
   actionable service-specific error, while opted-out services pass.
5. Add a test rejecting `rolling_update && destroy_before_deploy` before any
   command execution.
6. Run targeted tests and format checks.
7. Commit as `feat: parse compose rollout policy`.

## Task 3: Represent desired replicas without synthetic services

**Files:**

- Modify `client/core/rs/src/entities/stack.rs`
- Modify `bin/core/src/stack/services.rs`
- Modify `bin/periphery/src/api/compose.rs`
- Modify tests for service extraction and deploy responses
- Regenerate TypeScript types

**Steps:**

1. Add failing tests showing a service with `deploy.replicas: 3` produces one
   `StackServiceNames` entry with `desired_replicas == 3`, not three synthetic
   service names.
2. Add a backward-compatible `desired_replicas` field whose serde default is
   one.
3. Update both cached-content extraction and validated deploy responses to
   populate it.
4. Remove the existing branch that creates `service-1`, `service-2`, and
   similar synthetic service names.
5. Regenerate TypeScript types and run focused core/periphery tests.
6. Commit as `feat: model compose service replicas`.

## Task 4: Discover all replica containers by Compose labels

**Files:**

- Modify `client/core/rs/src/entities/stack.rs`
- Modify `bin/core/src/monitor/resources.rs`
- Modify `bin/core/src/helpers/query.rs`
- Add monitoring/state tests
- Regenerate TypeScript types

**Steps:**

1. Add failing tests with multiple containers sharing
   `com.docker.compose.project` and `com.docker.compose.service` labels.
2. Add `containers: Vec<ContainerListItem>` and replica summary fields to
   `StackService`, retaining the existing `container` field as the first
   matching container for compatibility.
3. Prefer Compose labels for matching; retain the current name regex only as a
   compatibility fallback for older/unlabelled imported containers.
4. Rewrite Stack state tests to compare ready/actual containers with
   `desired_replicas`, covering satisfied, missing, unhealthy, extra, stopped,
   and ignored services.
5. Ensure singleton behavior and Swarm responses remain unchanged.
6. Regenerate types and run focused core tests.
7. Commit as `feat: monitor compose replicas`.

## Task 5: Extract a testable rolling state machine

**Files:**

- Add `bin/periphery/src/api/compose/rollout.rs`
- Modify `bin/periphery/src/api/compose.rs`
- Add unit tests in the new module

**Steps:**

1. Define narrow traits or injected async callbacks for the operations the
   state machine needs: list service containers, scale up, inspect readiness,
   execute hook, stop/remove, fetch logs, and cleanup.
2. Add failing tests for old/new ID set detection and the invariant that
   exactly one new container appears after a surge.
3. Add failing state-machine tests for:
   - one singleton replacement;
   - sequential replacement of three replicas;
   - healthy readiness;
   - ten-second stable-running readiness without a healthcheck;
   - timeout cleanup;
   - hook failure cleanup;
   - cleanup failure preserving both errors;
   - failure after earlier replicas were replaced.
4. Implement the smallest state machine that passes the tests. Keep command
   construction and Docker execution outside the transition logic.
5. Verify that no old container is removed before its replacement is ready and
   its hook succeeds.
6. Run targeted periphery tests and format checks.
7. Commit as `feat: add compose rollout state machine`.

## Task 6: Implement Compose and Docker command adapters

**Files:**

- Modify `bin/periphery/src/api/compose/rollout.rs`
- Modify `bin/periphery/src/api/compose.rs`
- Reuse command helpers under `bin/periphery/src/docker` and command
  sanitization helpers
- Add command-construction tests

**Steps:**

1. Add failing tests for commands preserving project name, multiple `-f`
   files, multiple env files, run directory, wrapper, secret replacers, service
   name, `--no-deps`, `--no-recreate`, and `--scale service=N+1`.
2. Add validation for extra args that conflict with orchestration, including
   `--force-recreate`, `--always-recreate-deps`, `--scale`, and positional
   service selection.
3. Discover containers through Docker's Compose project/service labels and
   return stable IDs plus runtime/health data.
4. Implement bounded container-log capture for readiness failures.
5. Implement hook execution with the existing sanitized command path and
   `docker exec <id> sh -c <hook>`.
6. Ensure stop/remove and failed-new-container cleanup report independent
   errors without continuing destructive work.
7. Run targeted tests and format checks.
8. Commit as `feat: execute compose rolling updates`.

## Task 7: Integrate rolling deployment into ComposeUp

**Files:**

- Modify `bin/periphery/src/api/compose.rs`
- Modify relevant tests in `bin/periphery/src/main.rs` or colocated modules

**Steps:**

1. Add a regression test proving `rolling_update: false` constructs and runs
   the existing full Compose up path unchanged.
2. Add orchestration tests proving rolling validation happens after resolved
   config is available but before any container mutation.
3. Preserve write, validation, registry login, pre-deploy, build, and pull
   ordering.
4. Partition selected services into rolling, opted-out, and not-yet-running
   groups.
5. Update opted-out services individually with
   `compose up -d --no-deps <service>`.
6. Start services with no running containers normally at their desired replica
   count.
7. Run the state machine sequentially for rolling services.
8. Set `res.deployed` only after every selected service succeeds and run
   post-deploy only on full success.
9. Emit separate sanitized logs for validation, surge, readiness, hook,
   removal, cleanup, and final verification.
10. Run targeted periphery tests and format checks.
11. Commit as `feat: integrate compose rolling deploys`.

## Task 8: Add Stack configuration and replica UI

**Files:**

- Modify `ui/src/resources/stack/config/index.tsx`
- Modify `ui/src/resources/stack/services.tsx`
- Modify nearby reusable status/container display components only when needed

**Steps:**

1. Add a Compose-only `Rolling Update` switch with text explaining default
   per-service behavior, opt-out label, healthcheck behavior, and
   `container_name`/published-port restrictions.
2. Disable the switch or show an inline conflict when
   `destroy_before_deploy` is enabled, and apply the inverse guard to the
   destroy switch.
3. Show `actual / desired replicas` for replicated services.
4. Add an expandable container list showing each replica's state, health,
   image, networks, and ports while retaining compact singleton rows.
5. Check stable React keys use container IDs rather than service names or
   array indexes.
6. Run `rtk yarn --cwd ui build`.
7. Commit as `feat: show compose rolling updates and replicas`.

## Task 9: Add local integration proof

**Files:**

- Add `compose/rollout.local.compose.yaml`
- Add `scripts/compose-rollout/local.sh`
- Add `scripts/compose-rollout/local-wrapper.test.sh`
- Add `scripts/compose-rollout/verify-runtime.sh`
- Add `docs/development/compose-rollout-local.md`

**Steps:**

1. Create a two-replica HTTP fixture with a healthcheck and observable
   container identity.
2. Record ready replica count throughout rollout and assert it never reaches
   zero.
3. Assert all original IDs are replaced and final actual count equals desired
   count.
4. Add a failing-healthcheck case proving the new container is removed and the
   corresponding old container survives.
5. Add opt-out and incompatible-port/container-name cases.
6. Add a static wrapper test that stubs Docker and verifies project isolation,
   command construction, failure cleanup, and reset safeguards without a live
   daemon.
7. Run the harness against the local Docker context and write its secret-free
   proof to `target/compose-rollout/runtime-proof.json`; do not commit
   host-specific IDs or runtime output.
8. Commit as `test: verify compose rolling updates`.

## Task 10: Documentation and full verification

**Files:**

- Modify the Stack documentation in `docsite`
- Modify example Compose files only if a focused example already exists
- Update generated schema artifacts required by the repository

**Steps:**

1. Document enablement, replica declaration, opt-out, pre-stop draining,
   readiness defaults, incompatibilities, partial failure semantics, and the
   absence of an imperative scale control.
2. Include a minimal Traefik-compatible replicated service example without
   `container_name` or host-bound ports.
3. Regenerate TypeScript types and resource schemas, then confirm the worktree
   contains no unexplained generated diff.
4. Run:
   - `rtk cargo fmt --all -- --check`
   - targeted core and periphery tests
   - `rtk cargo test --workspace`; if an environment failure prevents it,
     retain the exact command and error alongside passing targeted tests
   - `rtk cargo build --workspace`
   - `rtk yarn --cwd client/core/ts build`
   - `rtk yarn --cwd ui build`
   - `rtk yarn --cwd docsite build`
   - the local integration harness
5. Review `rtk git diff --check`, `rtk git status --short`, and the complete
   branch diff for unrelated changes.
6. Commit documentation as `docs: document compose rolling updates`.

## Completion Criteria

- Existing Stacks deploy exactly as before unless `rolling_update` is enabled.
- A replicated service is represented once and all of its containers affect
  health and UI state.
- Enabled compatible services replace replicas sequentially with at most one
  surge container and without removing an old replica before readiness.
- Opted-out services use ordinary Compose deployment.
- Incompatible rolling services fail before mutation with actionable errors.
- Readiness and hook failures preserve the corresponding old replica and clean
  up the failed new replica when possible.
- Generated clients, UI, docs, targeted tests, workspace checks, and the local
  integration proof pass, or any environment-only blocker is reported with
  exact evidence.
