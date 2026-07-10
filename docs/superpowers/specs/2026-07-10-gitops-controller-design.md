# Pull-Based GitOps Controller Design

**Status:** Approved for implementation planning  
**Date:** 2026-07-10  
**Target:** `intezya/komodo` fork only

## Summary

Add an opt-in, pull-based GitOps controller to Komodo Core. The
controller periodically reads Git-backed Resource Sync and Stack
sources, applies safe Resource Sync changes, and reconciles running
Docker Compose stacks when their tracked files change.

The controller uses the existing
`KOMODO_RESOURCE_POLL_INTERVAL`. Our first deployment will set it to
`1-min`. Webhooks and CI workflows remain supported but are not
required for resources managed by the controller.

The feature is conservative by default:

- both new automation flags default to `false`;
- whole-resource deletions are never applied automatically;
- an existing Stack is automatically reconciled only while it is
  running;
- invalid Git, TOML, file, or Compose input does not trigger a runtime
  change;
- automatic full Compose reconciliation removes orphaned services,
  while manual deployment behavior remains unchanged.

## Goals

1. Let Komodo discover Git changes without GitHub Actions or webhooks.
2. Create and update resources declared by a Git-backed Resource Sync.
3. Create a Stack and deploy it only when its Resource Sync TOML entry
   explicitly has `deploy = true`.
4. Reconcile tracked files for an existing, running, Git-backed Stack.
5. Ignore commits that do not change files relevant to a Stack.
6. Preserve stopped or otherwise non-running existing Stacks.
7. Leave whole-resource deletions visible as pending manual work.
8. Fetch each effective Git source once per controller cycle.
9. Reuse Komodo's existing permissions, action locks, Updates, secret
   handling, and deployment machinery.
10. Document the controller as a first-class Komodo component.

## Non-Goals

- Direct Git reconciliation for Build or Repo resources.
- Docker image digest polling or image auto-update changes.
- Automatically deleting a Stack or any other Komodo resource.
- Treating deletion of a tracked file as deletion of a Komodo resource.
- Changing manual `RunSync`, manual Stack deploy, webhook, or image
  auto-update semantics.
- Automatically rolling back a Compose deployment that passed
  validation but failed during `docker compose up`.
- Replacing the existing Resource Sync TOML format.

A Resource Sync may continue to create or update any resource type it
already supports. The v1 limitation means that only Stack resources
receive a second-stage direct Git runtime reconciliation.

## Current Behavior and Gap

Core already runs `spawn_resource_refresh_loop()` on
`resource_poll_interval`. `RefreshStackCache` refreshes Git-backed
Stack contents and hashes, and `RefreshResourceSyncPending` computes
Resource Sync diffs. Neither operation applies the discovered Git
change.

`poll_for_updates` and `auto_update` are a separate image-digest
feature. `auto_pull` only pulls images before a deployment that was
already triggered. None of these fields polls Git for Compose changes.

`DeployStackIfChanged` already compares deployed and latest tracked
file contents and supports full deploy, full restart, and
service-scoped deploy/restart decisions. It does not currently remove
Compose orphans during a full deploy, and it refreshes its own Stack
source instead of consuming a shared source snapshot.

## Configuration

Two resource-level opt-in fields are added.

| Resource | Field | Default | Meaning |
|---|---|---:|---|
| Resource Sync | `auto_apply_updates` | `false` | Automatically apply safe Create and Update diffs from Git. |
| Stack | `auto_deploy_git_updates` | `false` | Reconcile relevant Git file changes while the existing Stack is running. |

Both fields belong to their resource config types, partial config
types, generated schemas, Rust and TypeScript clients, and UI forms.
They are serializable in Resource Sync TOML like all other config
fields.

Example controller-managed Resource Sync:

```toml
[[resource_sync]]
name = "de1"

[resource_sync.config]
git_account = "intezya"
repo = "intezya/komodo-gitops"
branch = "main"
resource_path = ["resources/de1.toml"]
auto_apply_updates = true
```

Example Stack declaration managed by that sync:

```toml
[[stack]]
name = "pocket-id"
deploy = false

[stack.config]
server = "de1"
project_name = "pocket-id"
git_account = "intezya"
repo = "intezya/komodo-gitops"
branch = "main"
run_directory = "stacks/de1/pocket-id"
file_paths = ["compose.yaml"]
auto_deploy_git_updates = true
```

With `deploy = false`, creation never starts this new Stack. Because
the controller only reconciles existing running Stacks, the Stack also
stays down on later cycles until it is deployed manually. Setting
`deploy = true` explicitly authorizes create-and-deploy.

The controller interval is configured globally:

```toml
resource_poll_interval = "1-min"
```

or:

```text
KOMODO_RESOURCE_POLL_INTERVAL=1-min
```

## Architecture

The new Core module is deliberately separate from the generic resource
refresh loop:

```text
bin/core/src/gitops/
  mod.rs       controller lifecycle, cycle guard, and phase ordering
  source.rs    effective source resolution and shared Git snapshots
  sync.rs      safe Resource Sync planning and application
  stack.rs     Stack change detection and runtime reconciliation
```

### Controller lifecycle

Core starts one `GitOpsController` task. It uses the same configured
`resource_poll_interval` as the existing resource refresh loop. A cycle
guard prevents a new cycle from starting while the prior cycle is
still running. The controller does not spawn overlapping reconciliation
for the same resource and all actual mutations continue through
Komodo's existing action-state locks.

The generic refresh loop continues to own Builds, Repos, files-on-host
resources, UI-defined resources, and Git-backed Stack/Resource Sync
resources whose new automation flag is disabled. It skips opted-in
Git-backed Stacks and Resource Syncs because the GitOps controller owns
their refresh. Thus the default-false configuration preserves current
behavior, while an opted-in source is not fetched again by the generic
loop.

### Source snapshots

At the start of a cycle, the controller loads opted-in Git-backed
Resource Syncs and Stacks and resolves linked Repo resources into an
effective Git source. Automatic polling only applies to moving branch
sources; a resource with a fixed `commit` remains on the generic refresh
path because it has no moving Git target to reconcile. Sources are
grouped by:

```text
(provider, account, repo, branch)
```

Each group is fetched once for discovery during that controller cycle.
The resulting immutable snapshot contains the checkout root, commit
hash, commit message, and any fetch error. Resource-specific parsing
then reads its own paths from the shared checkout.

The one-fetch guarantee covers controller discovery. An actual deploy
may still use the existing target-side repository preparation required
by `DeployStack` and Periphery.

A fetch failure is isolated to its source group. Other groups continue.
Every resource in the failed group keeps its current configuration and
runtime state and is retried on the next cycle.

### Cycle ordering

Each cycle has four ordered phases:

1. Load eligible resources and fetch grouped Git source snapshots.
2. Parse, diff, and safely apply opted-in Resource Syncs.
3. Reload Stack metadata so newly created or updated Stack resources
   are visible, then reconcile eligible running Stacks from the same
   source snapshots.
4. Persist refreshed pending/latest metadata and finish Updates for
   actions or errors.

Resource Sync runs first because its safe changes may create or update
the Stack resources considered in phase three.

A Stack created during phase two does not receive separate direct Stack
reconciliation until the next cycle. Its initial `deploy = true` action
is already handled by Resource Sync; `deploy = false` leaves it down.
This rule also prevents an unplanned second source fetch when a new
Stack points at a repository that was not present at cycle start.

## Resource Sync Reconciliation

The controller parses the configured `resource_path` values from its
shared source snapshot and computes the same Resource Sync diffs as the
manual pending-refresh path.

It partitions every diff by operation before mutation:

| Diff | Automatic behavior |
|---|---|
| Create | Apply, subject to collision checks. |
| Update | Apply. |
| Delete | Skip and retain as pending. |

The partition applies to resources, variables, and user groups. Safe
changes are not blocked merely because the same commit also contains a
Delete. Manual `RunSync` remains capable of applying the full pending
set according to existing Resource Sync configuration and permissions.

Implementation extracts the existing Run Sync planning and execution
logic so it can consume prepared remote resources and an explicit safe
operation filter. The controller must not call the public `RunSync`
resolver in a way that refetches the repository or accidentally
includes Delete operations. Existing dependency ordering between
resource types is preserved.

After a partial safe apply, pending state is recomputed from the same
snapshot. `last_sync_hash` may identify the commit whose safe subset
was applied, while `pending_hash` and the remaining diffs continue to
show the unapplied Delete operations.

### Stack creation and `deploy`

For a new Stack:

- `deploy = true` creates and deploys it using existing Resource Sync
  dependency ordering;
- `deploy = false` creates it without deployment;
- the new Stack is never deployed merely because
  `auto_deploy_git_updates = true`.

For an existing Stack, automatic Resource Sync and direct Stack Git
reconciliation only deploy when its current state is `Running`.
`Stopped`, `Paused`, `Created`, `Down`, `Unknown`, and unhealthy or
transitional states are left unchanged. Their latest Git metadata may
advance, so the difference remains visible and can be reconciled after
an operator starts or deploys the Stack.

### Collision guard

Before creating a Stack, the controller checks all known pending Stack
deletion diffs, including those belonging to other Resource Syncs. If a
proposed Stack would use the same effective deployment target and
Compose `project_name` as a Stack that is still pending deletion,
creation is skipped and remains pending. For the initial Compose
rollout this is specifically the `server_id + project_name` pair.

An empty `project_name` resolves to the Stack name before comparison.
The guard prevents two Komodo resources from controlling the same
Docker Compose project while automatic deletion is disabled.

## Stack Reconciliation

Only Git-backed Stacks with `auto_deploy_git_updates = true` are
eligible. Files-on-host and UI-defined Stacks are outside v1.

The relevant file set is derived from existing Stack configuration:

- Compose `file_paths`;
- the primary and additional environment files;
- additional config files and their `requires`/`services` metadata;
- the Stack `run_directory` used to resolve those paths.

The controller compares the contents of those files, not merely the
repository commit hash. A commit that only changes a README or another
untracked path updates source metadata but causes no deploy.

For a running Stack with valid changed content, the controller reuses
the existing `DeployStackIfChanged` decision model:

- changed global Compose or redeploy-required file: full deploy;
- changed restart-required global file: full restart;
- changed service-scoped file: targeted deploy or restart;
- no relevant content change: no action.

The public manual request remains unchanged. Its decision logic and
execution are extracted behind an internal entry point that accepts a
prepared Stack snapshot and automatic-reconciliation options. This
avoids a second discovery fetch and preserves the existing action lock,
permissions, Update records, secret interpolation, pre/post commands,
and success bookkeeping.

### Removed Compose services

When a full automatic Compose deployment is required, Core tells
Periphery to run `docker compose up -d --remove-orphans`. The
Core-to-Periphery `ComposeUp` request therefore gains default-false
`remove_orphans` and `validate_before_pre_deploy` options. Manual
deployments pass `false` for both. All controller-triggered deploys set
`validate_before_pre_deploy = true`; only a full automatic
reconciliation also sets `remove_orphans = true`.

Targeted service actions do not use `--remove-orphans`. Swarm behavior
is unchanged. This makes removal of a service from Compose declarative
without making whole-resource deletion automatic.

## Validation, Errors, and Retry

Validation happens before automatic runtime mutation:

1. Git fetch must succeed.
2. Every configured Resource Sync file must be readable and valid TOML.
3. Required Stack files must exist and be readable.
4. Compose content must parse sufficiently for Stack discovery.
5. In automatic reconciliation mode, Periphery's
   `docker compose config` preflight must succeed before pre-deploy,
   build, pull, `down`, or `up` commands. The manual deployment order
   remains unchanged.

If any preflight fails, the current runtime is not changed. The error
is stored through existing fields such as `pending_error`,
`remote_errors`, or Stack remote/missing-file metadata, and an error
Update is recorded where an execution was attempted. The resource is
retried on the next interval.

A failure during `docker compose up` does not advance deployed contents
or `deployed_hash`; the next cycle can retry. The controller does not
claim that runtime is unchanged after a deployment command has begun,
and it does not add automatic rollback in v1.

No Update is emitted for a cycle with no relevant change. Normal
action and error Updates remain the audit trail. A stopped Stack with a
pending Git change is represented by its latest/deployed metadata and
does not generate a repeated no-op Update every minute.

## Concurrency and Security

- Only one controller cycle runs at a time.
- Shared Git snapshots are immutable within a cycle.
- Per-resource mutation uses existing Komodo action-state guards.
- A busy resource is skipped or fails through the current lock path and
  is retried later; the controller does not bypass locks.
- Existing Git provider accounts and tokens are used. Tokens and
  interpolated secrets are never added to controller logs or snapshots.
- Enabling automatic Resource Sync application grants Git authors the
  ability to create and update the resource types allowed by that sync.
  Deletions remain a manual trust boundary.

## Observability

The controller logs cycle start/end, duration, source-group counts, and
per-source failures at normal Core log levels. It reuses existing
resource information:

- Stack `latest_hash`, `latest_message`, `deployed_hash`, and tracked
  contents;
- Resource Sync `pending_hash`, `pending_error`, `remote_errors`, and
  pending diffs;
- existing Komodo Updates for executions and failures.

Image digest availability remains a separate status and must not be
presented as a Git update.

## Documentation

Implementation includes:

- `docsite/docs/automate/gitops-controller.md` as the component design
  and operator guide, linked from the Docusaurus navigation;
- a link from the repository root README;
- module-level Rust documentation under `bin/core/src/gitops/`;
- field documentation in generated schemas and clients.

The operator guide covers purpose and boundaries, reconciliation flow,
the Create/Update/Delete matrix, Git versus image polling, stopped
Stacks, `--remove-orphans`, mixed syncs, collision handling, the
one-minute configuration, TOML examples, failure/retry behavior,
rollback limits, provider-token security, and migration away from
deploy-on-push workflows.

## Test Plan

Unit and integration-level tests cover:

1. Multiple Stacks and Resource Syncs sharing a source cause one
   discovery fetch per cycle.
2. A commit changing only irrelevant paths causes no Stack action.
3. A tracked Compose change selects `DeployStackIfChanged` behavior.
4. Removing a Compose service selects full automatic deploy with
   `--remove-orphans`.
5. Manual deploy and targeted service actions do not gain
   `--remove-orphans`.
6. An existing non-running Stack remains non-running with pending Git
   metadata.
7. Resource Sync Create and Update operations auto-apply.
8. Delete operations remain pending.
9. A mixed sync applies safe changes while retaining Delete diffs.
10. A `server_id + project_name` collision blocks Stack creation.
11. A new Stack with `deploy = true` is created and deployed.
12. A new Stack with `deploy = false` is created only.
13. Git, TOML, missing-file, and Compose validation failures do not
    start runtime mutation.
14. A failed action does not advance deployed metadata and is eligible
    on the next cycle.
15. Default-false flags preserve the existing refresh and deployment
    behavior.
16. The cycle guard prevents overlap.
17. Fixed-commit resources remain outside automatic polling.

Verification includes Rust formatting and targeted Core, client,
Periphery, schema-generation, and UI/client build checks appropriate to
the touched code.

## Rollout

1. Implement in an isolated worktree and keep changes inside
   `intezya/komodo`.
2. Publish fork Core/UI/Periphery images containing the feature.
3. Upgrade the Komodo installation with both new flags left `false`.
4. Set `KOMODO_RESOURCE_POLL_INTERVAL=1-min` and verify refresh load.
5. Enable `auto_apply_updates` on the `de1` Resource Sync.
6. Observe at least several successful controller cycles and audit
   records.
7. Enable `auto_deploy_git_updates` one Stack at a time.
8. Use the existing undeployed `pocket-id` Stack as the first migration
   candidate while Dokploy remains the live owner.
9. Perform and verify the Pocket ID ownership cutover separately.
10. Remove deploy-on-push GitHub workflows only after native polling is
    proven for the relevant Stack paths.

Rollback is configuration-first: disable both opt-in flags. The generic
refresh loop resumes ownership on the next refresh cycle, and existing
manual/webhook operations remain available.
If required, roll back the Komodo images after the flags are disabled.

## Acceptance Criteria

The feature is ready when a Git commit can safely create/update
Resource Sync-managed resources and reconcile a running Compose Stack
within one configured polling interval, without a webhook; irrelevant
commits do nothing; stopped Stacks stay stopped; removed Compose
services are removed during automatic full reconciliation; resource
deletions remain pending; and all of this is disabled by default and
documented.
