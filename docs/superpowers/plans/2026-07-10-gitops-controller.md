# Pull-Based GitOps Controller Implementation Plan

**Design:**
`docs/superpowers/specs/2026-07-10-gitops-controller-design.md`

**Goal:** Add opt-in pull-based reconciliation for Git-backed Resource
Syncs and running Git-backed Compose Stacks without changing manual,
webhook, or image-digest auto-update behavior.

**Implementation boundary:** This plan changes the Komodo fork only. It
does not enable the feature on a live instance, publish images, remove a
workflow, or cut Pocket ID over from Dokploy.

## Execution Rules

- Work in the existing `gitops-controller-design` worktree or create a
  new implementation worktree from its approved commits.
- Keep every new flag `serde(default)` and default it to `false`.
- Write the focused test first for each behavior, confirm it fails for
  the expected reason, then implement the smallest passing change.
- Preserve public Core request semantics. New Core-to-Periphery fields
  must be backward-compatible with `serde(default)`.
- Do not bypass Resource Sync or Stack action-state guards.
- Never include Git tokens, environment contents, or interpolated
  secrets in test output or logs.
- Use `rtk` for every shell command.
- Keep PRs inside `intezya/komodo`.

## Task 1: Add the opt-in config fields

**Files**

- Modify `client/core/rs/src/entities/sync.rs`.
- Modify `client/core/rs/src/entities/stack.rs`.
- Regenerate `client/core/ts/src/types.ts`.

**Steps**

1. Add a serialization/default test near each config type proving that
   an old config without the new field deserializes to `false`.
2. Run the focused client tests and confirm the fields do not yet exist.
3. Add `ResourceSyncConfig::auto_apply_updates: bool` with documentation,
   `#[serde(default)]`, and `#[builder(default)]`; add it to `Default`.
4. Add `StackConfig::auto_deploy_git_updates: bool` in the Git source
   section with the same default and documentation treatment; add it to
   `Default`.
5. Regenerate TypeScript types with:

   ```bash
   rtk node client/core/ts/generate_types.mjs
   ```

6. Verify:

   ```bash
   rtk cargo test -p komodo_client
   rtk yarn --cwd client/core/ts build
   rtk cargo run -p xtask -- generate resource-schema --pretty --stdout
   ```

7. Commit as `feat: add gitops resource flags`.

## Task 2: Model effective Git sources and shared snapshots

**Files**

- Create `bin/core/src/gitops/mod.rs`.
- Create `bin/core/src/gitops/source.rs`.
- Modify `bin/core/src/lib.rs` only to declare the module; do not start
  the controller yet.
- Reuse `client/core/rs/src/entities/mod.rs` for
  `RepoExecutionArgs`; avoid changing its public shape unless required.

**Steps**

1. Add pure unit tests for:

   - direct Stack and Resource Sync Git config resolving to an effective
     source;
   - `linked_repo` resolving through a supplied Repo map;
   - source identity grouping by `(provider, account, repo, branch)`;
   - multiple consumers of one source producing one fetch request;
   - fixed-commit, files-on-host, UI-file, and empty-repo resources being
     ineligible.

2. Introduce small internal types:

   - `GitSourceKey` for the approved identity tuple;
   - `GitSourceConsumer` identifying Stack or Resource Sync plus id;
   - `GitSourceSnapshot` containing checkout root, hash, message, and a
     sanitized error;
   - an eligibility result that distinguishes ineligible resources from
     invalid linked Repo configuration.

3. Resolve `linked_repo` before constructing the key. Treat a fixed
   `commit` as ineligible for automatic polling.
4. Implement a snapshot builder that receives a fetch function/closure
   so a unit test can count calls without network access. Production
   fetch uses the existing token lookup and `git::pull_or_clone` path.
5. Fetch source groups sequentially in v1. Existing cache paths are not
   account-qualified, so sequential operation avoids concurrent writes
   to one checkout while preserving the one-fetch-per-group guarantee.
6. Ensure token values never enter `Debug`, errors, or the snapshot.
7. Verify:

   ```bash
   rtk cargo test -p komodo_core gitops::source
   rtk cargo check -p komodo_core
   ```

8. Commit as `feat: add gitops source snapshots`.

## Task 3: Let Stack and Resource Sync readers consume a snapshot

**Files**

- Modify `bin/core/src/stack/remote.rs`.
- Modify `bin/core/src/sync/remote.rs`.
- Modify `bin/core/src/api/write/stack.rs`.
- Modify `bin/core/src/api/write/sync.rs`.
- Modify `bin/core/src/gitops/source.rs` if a shared prepared-source type
  is needed.

**Steps**

1. Add focused tests for reading different Stack run directories and
   Resource Sync paths from one temporary/prepared checkout abstraction.
   Keep filesystem mechanics behind a small reader function so tests do
   not require Git or Mongo.
2. Extract the current post-fetch Stack logic into a function accepting
   `(stack, checkout_root, hash, message)`. It must return the existing
   `RemoteComposeContents`, including missing and errored files.
3. Extract the current post-fetch Resource Sync logic into a function
   accepting `(sync, checkout_root, hash, message)`. It must return the
   existing file/error data and parsed `ResourcesToml`.
4. Keep `get_repo_compose_contents()` and `get_remote_resources()` as
   compatibility wrappers: fetch as today, then call the prepared reader.
   Manual refresh/write APIs must behave exactly as before.
5. Add internal persistence helpers so the controller can update Stack
   latest metadata and Resource Sync pending metadata from a prepared
   result without triggering another Git fetch.
6. Test missing files and invalid TOML: return structured errors and do
   not overwrite the last successful deployed state.
7. Verify:

   ```bash
   rtk cargo test -p komodo_core stack::remote
   rtk cargo test -p komodo_core sync::remote
   rtk cargo check -p komodo_core
   ```

8. Commit as `refactor: accept prepared git sources`.

## Task 4: Extract safe Resource Sync execution

**Files**

- Modify `bin/core/src/sync/mod.rs`.
- Modify `bin/core/src/sync/execute.rs`.
- Modify `bin/core/src/api/execute/sync.rs`.
- Modify `bin/core/src/api/write/sync.rs`.
- Create `bin/core/src/gitops/sync.rs`.

**Steps**

1. Add pure tests for an execution filter proving:

   - Create and Update deltas remain;
   - every resource Delete delta is removed;
   - variable and user-group deletes are removed;
   - a mixed set retains safe work rather than rejecting the whole set;
   - manual mode retains all deletes.

2. Add an internal `SyncExecutionMode` with `Manual` and
   `GitOpsSafe` variants. Keep it out of the public client API.
3. Extract the body of `RunSync::resolve` after permission/action setup
   into a helper that accepts prepared `ResourcesToml`, source metadata,
   execution filters, and an existing Update context.
4. Build full deltas using existing logic, then strip only delete lists
   in `GitOpsSafe` mode. Do not pass `delete = false` during diffing,
   because pending Delete information is still needed for visibility
   and collision checks.
5. Preserve the current resource-type dependency order and the existing
   `ExecuteResourceSync` implementations.
6. After a safe partial apply, recompute pending state from the same
   prepared snapshot. Leave Delete diffs in `ResourceSyncInfo` and do
   not refetch Git.
7. Keep public `RunSync` on `Manual` mode and its current fetch path.
8. Return a small internal result containing created resource ids/names,
   applied operations, skipped deletes, and errors. The controller uses
   it for same-cycle decisions; it is not added to the public API.
9. Verify:

   ```bash
   rtk cargo test -p komodo_core sync::
   rtk cargo check -p komodo_core
   ```

10. Commit as `refactor: add safe resource sync mode`.

## Task 5: Enforce collision and automatic deploy policy

**Files**

- Modify `bin/core/src/gitops/sync.rs`.
- Modify `bin/core/src/sync/deploy.rs`.
- Modify `bin/core/src/api/execute/sync.rs` only where it calls the
  extracted deploy helper.

**Steps**

1. Add pure tests for effective Compose ownership:

   - empty `project_name` resolves to Stack name;
   - same `server_id + project_name` as any pending Stack deletion blocks
     a proposed Create, including a deletion from another Resource Sync;
   - different server or project does not block creation.

2. Load pending Stack Delete diffs from all Resource Sync records once
   per cycle. Parse only the required current Stack TOML fields and fail
   closed if a would-be conflicting deletion cannot be interpreted.
3. Filter conflicting Stack creates before calling
   `Stack::execute_sync_updates`; leave them visible in recomputed
   pending state with a useful sanitized reason.
4. Add an internal deploy policy to `build_deploy_cache` and
   `deploy_from_cache`:

   - manual policy preserves current behavior;
   - GitOps policy deploys a newly created Stack only when its TOML has
     `deploy = true`;
   - GitOps policy may redeploy an existing Stack only in `Running`;
   - existing non-running Stacks remain pending and are not started.

5. Do not change Deployment deploy behavior except where required to
   pass the new internal policy through; v1 direct Git reconciliation
   remains Stack-only.
6. Verify:

   ```bash
   rtk cargo test -p komodo_core gitops::sync
   rtk cargo test -p komodo_core sync::deploy
   ```

7. Commit as `feat: enforce safe gitops sync policy`.

## Task 6: Add automatic Compose execution options

**Files**

- Modify `client/periphery/rs/src/api/compose.rs`.
- Modify `bin/periphery/src/api/compose.rs`.
- Modify `bin/core/src/api/execute/stack.rs` call sites.

**Steps**

1. Extend Periphery `ComposeUp` with default-false
   `remove_orphans` and `validate_before_pre_deploy` fields.
2. Add command-construction tests proving:

   - full automatic up includes `--remove-orphans`;
   - targeted service up never includes `--remove-orphans`;
   - force-recreate behavior remains unchanged;
   - default/manual up remains byte-for-byte equivalent.

3. Extend `compose_up_command()` with an explicit orphan option instead
   of injecting it through user `extra_args`.
4. Extract Compose config validation into a helper that can run either
   before or after pre-deploy without duplicating the command. In
   automatic mode it must finish successfully before pre-deploy, build,
   pull, down, or up. In default/manual mode preserve the current order.
5. Add a stage-order unit test around a pure execution-stage planner or
   equivalent helper; do not require Docker in unit tests.
6. Pass both fields as `false` from all existing manual Core paths.
   Leave Swarm request behavior unchanged.
7. Verify:

   ```bash
   rtk cargo test -p komodo_periphery api::compose
   rtk cargo check -p periphery_client -p komodo_periphery -p komodo_core
   ```

8. Commit as `feat: add safe compose reconcile options`.

## Task 7: Reconcile a prepared running Stack

**Files**

- Modify `bin/core/src/api/execute/stack.rs`.
- Create `bin/core/src/gitops/stack.rs`.
- Modify `bin/core/src/gitops/mod.rs` exports as needed.

**Steps**

1. Expand tests around `resolve_deploy_if_changed_action()` for:

   - unchanged tracked files;
   - changed Compose content producing full deploy;
   - service-scoped redeploy and restart;
   - removal of a service from Compose still producing full deploy;
   - a new tracked file producing full deploy.

2. Extract an internal Stack-if-changed executor with options for:

   - whether it refreshes Git itself;
   - whether a full deploy removes orphans;
   - whether Compose validates before pre-deploy.

3. Keep public `DeployStackIfChanged` on the existing refresh behavior
   with both automatic Compose options disabled.
4. Have `gitops::stack` consume the already persisted/prepared Stack
   snapshot, read state from `stack_status_cache`, and return without an
   Update unless state is exactly `Running`.
5. For a running Stack:

   - full deploy sets both automatic Compose options;
   - targeted deploy validates early but does not remove orphans;
   - restart-only paths preserve current behavior;
   - no relevant content change creates no Update.

6. Ensure deployed contents/hash advance only through the existing
   successful deployment bookkeeping. A failed deployment remains
   eligible next cycle.
7. Verify:

   ```bash
   rtk cargo test -p komodo_core api::execute::stack
   rtk cargo test -p komodo_core gitops::stack
   ```

8. Commit as `feat: reconcile prepared git stacks`.

## Task 8: Wire the non-overlapping controller cycle

**Files**

- Modify `bin/core/src/gitops/mod.rs`.
- Modify `bin/core/src/resource/refresh.rs`.
- Modify `bin/core/src/lib.rs`.
- Use `bin/core/src/state.rs` only if a shared controller lock/cache
  cannot stay private to `gitops`.

**Steps**

1. Add tests around a cycle coordinator with fake repositories and
   fake reconcilers proving:

   - a second cycle cannot overlap the first;
   - one source failure does not block another source group;
   - Resource Sync phase runs before Stack phase;
   - existing Stack ids are reloaded after sync updates;
   - Stacks created during this cycle are excluded from direct Stack
     reconciliation until the next cycle;
   - a Stack moved to a source not fetched at cycle start is deferred;
   - retry is possible on the next cycle after an error.

2. Implement `spawn_gitops_controller()` using
   `core_config().resource_poll_interval` and a private `Mutex<()>` with
   `try_lock()` to skip overlapping ticks.
3. At cycle start load Repo resources plus opted-in Stack and Resource
   Sync records, resolve eligibility, record the set of pre-existing
   Stack ids, group sources, and fetch snapshots once.
4. Run opted-in Resource Sync reconciliation sequentially. Then reload
   existing Stack records, retain only ids present at cycle start, and
   reconcile eligible running Stacks whose source snapshot exists.
5. Update `spawn_resource_refresh_loop()` ownership:

   - keep current handling for all resources when flags are false;
   - skip only moving-branch Git Stack/Resource Sync resources owned by
     the controller;
   - continue generic refresh for fixed-commit, files-on-host,
     UI-defined, invalid, and non-opted-in resources;
   - continue refreshing Builds and Repos exactly as today.

6. Start the new controller from `bin/core/src/lib.rs` next to the
   generic refresh loop.
7. Log cycle duration, consumer/group counts, and sanitized group
   failures. Do not emit an Update for no-op or stopped Stack cycles.
8. Verify:

   ```bash
   rtk cargo test -p komodo_core gitops::
   rtk cargo test -p komodo_core resource::refresh
   rtk cargo check -p komodo_core
   ```

9. Commit as `feat: run pull-based gitops controller`.

## Task 9: Expose the controls in the UI

**Files**

- Modify `ui/src/resources/sync/config.tsx`.
- Modify `ui/src/resources/stack/config/index.tsx`.
- Use the regenerated `client/core/ts/src/types.ts` from Task 1.

**Steps**

1. Add `Auto Apply Git Updates` to the Git Repo mode of Resource Sync.
   Explain that Create/Update are automatic, Delete remains pending,
   and Git authors gain configuration write authority.
2. Disable or hide the toggle for files-on-host, UI-file, and
   fixed-commit modes. If a previously enabled resource becomes
   ineligible, show the stored value but explain why it is inactive;
   do not silently mutate it.
3. Add `Deploy Git Updates` to the Stack Git Repo mode, separate from
   the existing image `Auto Update` group. Explicitly say that only a
   running Stack is reconciled and that image polling is unrelated.
4. Keep both controls permission-aware through the existing config form
   machinery.
5. Verify:

   ```bash
   rtk yarn --cwd client/core/ts build
   rtk yarn --cwd ui build
   ```

6. Manually inspect both forms in light and dark themes if a local UI is
   available; screenshots are required only if the PR review process
   asks for them.
7. Commit as `feat: expose gitops controls in ui`.

## Task 10: Write operator and module documentation

**Files**

- Create `docsite/docs/automate/gitops-controller.md`.
- Modify `docsite/sidebars.ts`.
- Modify `docsite/docs/automate/sync-resources.md`.
- Modify `docsite/docs/deploy/compose.md`.
- Modify `README.md`.
- Add module-level Rust docs to `bin/core/src/gitops/mod.rs`,
  `source.rs`, `sync.rs`, and `stack.rs`.

**Steps**

1. Write the operator guide from the approved design, including:

   - controller scope and one-minute configuration;
   - Git polling versus image digest polling;
   - opt-in flags and TOML examples;
   - Create/Update/Delete matrix;
   - mixed sync and collision behavior;
   - `deploy = true` versus `false` for new Stacks;
   - running/non-running behavior;
   - relevant paths and `--remove-orphans`;
   - errors, retry, rollback limits, provider tokens, and migration from
     deploy-on-push workflows.

2. Link the page in the Automate sidebar, Sync Resources docs, Compose
   docs, and root README.
3. Keep module docs focused on ownership and invariants rather than
   restating implementation line by line.
4. Verify:

   ```bash
   rtk yarn --cwd docsite build
   rtk cargo doc -p komodo_core --no-deps
   ```

5. Commit as `docs: document gitops controller`.

## Task 11: Full verification and fork-only PR

**Steps**

1. Regenerate TypeScript types once more and confirm a clean diff:

   ```bash
   rtk node client/core/ts/generate_types.mjs
   rtk git status --short
   ```

2. Run formatting and whitespace checks:

   ```bash
   rtk cargo fmt --all -- --check
   rtk git diff --check
   ```

3. Run Rust verification with incremental compilation disabled if this
   machine hits the known incremental compiler failure:

   ```bash
   rtk proxy env CARGO_INCREMENTAL=0 cargo test -p komodo_client
   rtk proxy env CARGO_INCREMENTAL=0 cargo test -p komodo_core
   rtk proxy env CARGO_INCREMENTAL=0 cargo test -p komodo_periphery
   rtk proxy env CARGO_INCREMENTAL=0 cargo check -p periphery_client
   ```

4. Run generated-client, UI, schema, and docs verification:

   ```bash
   rtk yarn --cwd client/core/ts build
   rtk yarn --cwd ui build
   rtk cargo run -p xtask -- generate resource-schema --pretty --stdout
   rtk yarn --cwd docsite build
   ```

5. Review the final diff against the design acceptance criteria. In
   particular, search for every construction of `ComposeUp` and verify
   existing paths explicitly preserve default/manual behavior.
6. Create a fork-only PR targeting `intezya/komodo:main`. Include the
   design link, behavior matrix, verification commands, and a clear note
   that the flags default to false.
7. Verify the PR target through the GitHub API before merge. Do not open
   any PR against the upstream/source repository.

## Post-Merge Rollout Checklist

This section is intentionally not part of implementation execution and
requires a separate operational go-ahead.

1. Publish fork Core, UI, and Periphery images.
2. Upgrade Komodo with both flags disabled and verify existing behavior.
3. Set `KOMODO_RESOURCE_POLL_INTERVAL=1-min`.
4. Enable `auto_apply_updates` only for `de1` and observe several cycles.
5. Enable `auto_deploy_git_updates` one Stack at a time.
6. Use the undeployed `pocket-id` Stack as the first ownership-migration
   candidate while Dokploy remains live.
7. Cut ownership over and remove deploy workflows only after runtime
   verification succeeds.
