# Komodo P1 Performance Program Design

**Status:** Approved for implementation-plan drafting on 2026-07-10

## Context

The performance audit of `main@1334baee` found no proven P0 issue, but it
identified fifteen actionable P1 findings. The findings do not belong in one
implementation plan: they span MongoDB access, Core and Periphery runtime
behavior, frontend delivery, and the build pipeline. A single plan would mix
different owners, validation environments, rollback strategies, and files.

This design groups the P1 findings into four umbrella implementation plans.
Each plan must remain independently reviewable, but it may contain ordered,
independently deployable PR checkpoints when its failure domains differ. The
cross-plan merge gates in this document are part of the design rather than
optional scheduling advice. P2 and P3 findings are out of scope unless a
narrowly identified dependency is required to complete a P1 fix; such a
dependency must be called out rather than silently absorbed.

## Goals

- Produce four Superpowers implementation plans, each with one coherent
  performance outcome.
- Tie every P1 audit finding to exactly one primary plan.
- Require a failing test, benchmark, trace, or measured baseline before each
  behavior-changing optimization.
- Preserve authorization, audit, update-delivery, and disconnected-client
  semantics while reducing repeated work.
- Give every plan explicit PR checkpoints, compatibility gates, and rollback
  boundaries; do not force an umbrella plan into one oversized PR.

## Non-Goals

- Implement any optimization while drafting these plans.
- Include unmeasured micro-optimizations such as alternative hashers, broad
  clone removal, or release-profile tuning.
- Fold the wider P2/P3 backlog into the P1 program.
- Assume production MongoDB indexes match repository bootstrap code; live
  indexes must be inspected before an index migration is proposed.
- Promise a percentage speedup without a representative workload measurement.

## Considered Decompositions

### Two plans: product runtime and engineering delivery

This minimizes document count but creates a very large product-runtime plan
covering Rust, MongoDB, WebSockets, and React. It is difficult to review,
parallelize, or roll back safely.

### Three plans: backend, frontend, and delivery

This is clearer, but the backend plan still combines data-access redesign with
Tokio blocking, event amplification, log bounds, and client polling. Those
changes have different invariants and verification methods.

### Four plans: data paths, runtime/events, UI, and delivery

This is the selected design. It keeps shared files and success metrics together
without creating a plan per individual function. The first two plans have an
explicit ordering; the UI and delivery plans can otherwise progress in
parallel.

## Plan 1: Core Data Paths and Cache Efficiency

**Plan file:**
`docs/superpowers/plans/2026-07-10-komodo-core-data-path-performance.md`

### Outcome

Core read and refresh costs should scale with the changed or requested data,
not repeatedly with the entire resource inventory.

### P1 findings assigned to this plan

1. Non-admin `ListUpdates` and `ListAlerts` permission fan-out across eleven
   resource types.
2. Monitoring relationship filters without repository-managed indexes on
   `config.server_id` and `config.swarm_id`.
3. Minute-based Build, Repo, Action, and Procedure state refresh N+1 queries.
4. Eleven sequential full-collection reads in `AllResources` every fifteen
   seconds and after resource mutations.

### Target architecture

- Load user groups once per authorization operation and reuse a permission
  snapshot keyed by `(user_id, permission_generation)`.
- Store `{ generation, mutation_in_progress }` in one authoritative MongoDB
  document. Every permission-changing write must go through a central helper
  that atomically acquires the mutation guard, sets
  `mutation_in_progress = true`, and advances the generation before changing
  permission data. While the flag is true, every Core bypasses snapshots. After
  the permission write succeeds, the helper advances the generation again and
  clears the flag. A failed or abandoned mutation leaves caching disabled until
  an explicit recovery path verifies authoritative state and clears the guard.
- Acquire and finalize the mutation guard with compare-and-swap against the
  expected generation. This serializes concurrent permission mutations; a
  competing writer retries from the new generation rather than modifying data
  under another writer's guard.
- Read the authoritative generation at the start and immediately before
  consumption of each request or WebSocket delivery batch, not once per
  resource type or subscriber. Each Core process keeps only generation-keyed
  local snapshots; therefore a generation change invalidates snapshots across
  Core instances without relying on process-local events.
- Immediately before an authorization result is consumed, read the guard a
  second time. The result may be used only if both reads observed the same
  generation with `mutation_in_progress = false`; otherwise retry from the new
  generation or bypass to authoritative reads while the guard is set. This
  second successful read is the authorization linearization point: a revocation
  whose final compare-and-swap occurs afterward is ordered after that request,
  while a revocation finalized earlier forces a retry.
- Authorize a targeted query against only the required resource type when the
  request already identifies that type.
- Fetch monitoring resources in collection-sized batches and group them by
  server or swarm in memory instead of issuing relationship queries per target.
- Replace per-resource latest-update reads with at most one aggregation query
  per resource type per refresh cycle, or an event-maintained state snapshot
  whose repair path obeys the same query budget.
- Split `AllResources` into independently replaceable type snapshots and update
  affected types incrementally. Keep a slower repair refresh for convergence.
- Publish each type snapshot atomically. If the database write succeeds but a
  cache update fails, return the authoritative write result, mark that type
  dirty, bypass it through a database read-through path, and schedule immediate
  repair. Do not report a post-commit cache error that encourages a duplicate
  write retry.

### Safety constraints

- Permission cache invalidation must fail closed. Stale permissions must never
  grant access after a user, group, or resource permission is revoked.
- Roll out permission caching disabled by default. Create the generation
  document and deploy guarded mutation code to every Core instance before
  enabling snapshot reads. Rollback begins by activating the kill switch before
  any old Core instance is allowed to serve permission mutations.
- The implementation plan must enumerate and test every mutation category that
  changes effective permissions: direct user permissions, group permissions,
  user-group membership, resource-group membership, group deletion, user
  deletion, and resource deletion. A runtime kill switch must force
  authoritative permission reads without a restart.
- Snapshot TTL is only a memory/repair backstop and must not be the correctness
  mechanism. A missing or unreadable generation, failed invalidation, or partial
  permission mutation always bypasses the cache.
- Index work must start with `getIndexes()` and
  `explain("executionStats")`; migrations must not duplicate existing manual
  production indexes.
- Cache repair must remain possible after a missed event or partial failure.
- Resource mutations must not report success before the authoritative database
  write succeeds.

### Success criteria

- A non-admin update/alert request performs no repeated group reads or full
  resource-list reads per resource type.
- Relationship queries show `IXSCAN` and
  `totalDocsExamined / max(nReturned, 1) <= 5` on the representative staging
  fixture.
- Each state-refresh cycle uses a fixed number of queries per resource type,
  rather than one or two queries per resource.
- A single resource mutation refreshes only affected cache state and no longer
  waits for eleven full collection reads.
- A failed post-commit cache update enters read-through mode immediately and
  converges to a clean snapshot within five seconds on the designated staging
  workload.
- Authorization regression tests cover grant, inherited grant, revocation, and
  group membership change across two Core processes, including a forced
  generation-update failure and the cache kill switch.

### Verification design

- Add Mongo command-count instrumentation around the affected endpoints and
  refresh loops.
- Benchmark admin and non-admin requests at 1, 100, and 1,000 resources.
- Capture pre/post execution stats for every proposed relationship or compound
  index.
- Add deterministic cache convergence tests for incremental update and repair
  refresh paths.
- Before behavior changes begin, freeze numeric budgets in the implementation
  plan from the instrumented baseline. At minimum, cached non-admin
  `ListUpdates`/`ListAlerts` must use no more than four MongoDB commands after
  request authentication, and a cold snapshot must remain constant with
  respect to resource-type and resource counts.

### Ordered PR checkpoints

1. Add measurements, inspect live indexes, and add only verified missing
   indexes.
2. Batch monitoring and latest-state reads without changing cache semantics.
3. Introduce atomic per-type `AllResources` snapshots, dirty read-through, and
   repair behavior.
4. Introduce the Mongo-backed permission generation, request-scoped batching,
   cross-Core tests, and kill switch.

## Plan 2: Runtime Backpressure and Update Event Pipeline

**Plan file:**
`docs/superpowers/plans/2026-07-10-komodo-runtime-backpressure-events.md`

### Outcome

Core and Periphery should remain responsive under API-key bursts, large
procedure output, batch operations, and large command output. Work must be
bounded by explicit concurrency and memory budgets.

### P1 findings assigned to this plan

1. Procedure progress clones and replaces the growing Update document for each
   output line, followed by per-subscriber authorization reads.
2. Synchronous bcrypt verification on Tokio workers and synchronous Periphery
   `sysinfo` refresh under a write lock.
3. Wide `join_all` and spawned-task fan-out without a global or per-server
   concurrency budget.
4. Stack and Swarm log paths accepting arbitrary tails while command output is
   fully buffered in memory.
5. The blocking Rust client polling `GetUpdate` without sleep, timeout, or
   backoff.

### Target architecture

- Keep the existing `Update` document and event envelope compatible, but cap
  persisted Update log data at 8 MiB so the document retains headroom below
  MongoDB's 16 MiB document limit. Flush append batches of at most 64 KiB or
  every 250 ms, whichever comes first. After the cap, persist one deterministic
  truncation marker, keep draining producer output without retaining it, and
  continue writing terminal status and error metadata.
- Coalesce progress notifications while guaranteeing an uncoalesced terminal
  event. Add optional `{ stream_epoch, sequence }` metadata to the existing
  event envelope so old UI versions continue to parse it. `stream_epoch` is a
  fresh identifier for one Core process's Update broadcast stream and changes
  on Core restart or when a client reconnects to another Core instance.
  `sequence` is an atomic, strictly increasing counter within that epoch and is
  assigned to every broadcast Update event before fan-out.
- Reuse the permission snapshot from Plan 1 for WebSocket delivery, with safe
  invalidation and a documented fallback when the snapshot is unavailable.
- Move bcrypt and system-information collection to bounded blocking execution.
  Publish short-lived immutable stats snapshots instead of holding locks across
  Docker or compose awaits.
- Add separate user-work and monitoring concurrency budgets at batch,
  monitoring, transport, and Docker process fan-out boundaries. Acquire global
  permits before per-server permits. User work waits in a bounded queue until a
  declared deadline and then returns an explicit overload result; monitoring
  uses non-blocking acquisition and reschedules skipped work. Permit ownership
  must be cancellation-safe and released on every error or panic path.
- Replace `Command::output()` on affected large-output paths with bounded pipe
  readers or ring buffers before allocation. Apply an 8 MiB byte cap, existing
  line caps where applicable, explicit truncation markers, and deterministic
  child cancellation/draining on timeout.
- Make the existing blocking client poll no faster than the async client's
  500 ms cadence. Add a separate timeout-capable API without changing the
  existing method's return type or forcing a new timeout on current callers.

### Safety constraints

- Coalescing must not lose terminal status, error details, or required audit
  history.
- The fixed 8 MiB persisted-log policy intentionally replaces the unsafe
  unbounded behavior. Old Core/UI combinations continue to see the existing log
  field and the textual truncation marker; no chunk collection or storage
  migration is introduced in this P1 program.
- Bcrypt cost and credential semantics must not change in this plan.
- Semaphores must not create cross-server head-of-line blocking; use both global
  and per-server limits where required.
- Monitoring and user-triggered work must never share a queue in which monitor
  fan-out can starve user work. The implementation plan must state permit
  counts, queue capacity, acquisition deadline, and overload behavior for each
  boundary before production code changes begin.
- Log truncation must be explicit to callers and must not silently turn a
  failed command into success.
- Existing async Rust client behavior must remain backward compatible.

### Success criteria

- Bytes written for procedure progress grow linearly until the 8 MiB persisted
  cap and remain constant after the truncation marker.
- Database reads per WebSocket event do not grow with subscriber count for
  cached authorization data.
- On the designated staging host, Tokio p99 event-loop lag stays below 10 ms
  during API-key concurrency tests and Periphery stats refreshes.
- Active task, Docker process, and per-server request counts never exceed the
  configured internal budgets.
- A producer emitting 100 MiB leaves the persisted Update below 9 MiB and
  increases peak process RSS by no more than 32 MiB above the idle baseline.
- The blocking client performs no more than two polls per second; the additive
  timeout API exits at its declared deadline.

### Verification design

- Test procedure output at 1, 10, and 100 MiB while recording Mongo bytes,
  document size, event count, and peak RSS. The 10 and 100 MiB cases must show a
  truncation marker and preserved terminal status.
- Load-test API-key authentication at concurrency 1, 8, and 32 while recording
  p95/p99 latency and Tokio busy time.
- Exercise batch sizes 10, 100, and 1,000 and assert maximum active tasks and
  processes.
- Add log-limit tests for line cap, byte cap, truncation marker, timeout, and
  command-failure preservation.
- Add blocking-client tests with a deterministic fake clock or bounded polling
  harness.

### Rolling compatibility

- New Core plus old UI: preserve the existing event envelope and guarantee
  start/final events; optional epoch/sequence metadata is ignored safely.
- Old Core plus new UI: keep a slow safety poll and treat missing sequence
  metadata as a reason to retain that fallback.
- New Core plus new UI: compare sequence values only within one epoch. Ignore
  duplicates or lower values, run a full synchronization barrier on a sequence
  gap or epoch change, then reset comparison state. The safety poll remains a
  last-resort convergence mechanism.
- No storage dual-read is required because bounded logs remain in the existing
  Update field. Rollback restores the old writer without a data migration;
  already truncated entries remain explicitly marked.

### Ordered PR checkpoints

1. Move bcrypt and `sysinfo` work off Tokio workers and shorten lock scopes.
2. Add blocking-client cadence, bounded pipe readers, byte caps, timeout
   draining, and RSS tests.
3. Add separate user/monitor queues and global/per-server concurrency budgets.
4. Add bounded Update append batches, event coalescing, optional sequence
   metadata, and permission-snapshot consumption after Plan 1 merges.

## Plan 3: UI Startup and Background Traffic

**Plan file:**
`docs/superpowers/plans/2026-07-10-komodo-ui-startup-background-traffic.md`

### Outcome

An authenticated page should load only the code required for its current route
and should not continuously refetch data for closed or hidden features.

### P1 findings assigned to this plan

1. The initial production graph eagerly includes Monaco, Monaco YAML,
   Recharts, xterm, and Prettier through `main.tsx`, the WebSocket provider, and
   the static resource registry.
2. Closed OmniSearch, dashboard summaries, responsive card subtrees, alerts,
   and update badges create high idle polling volume and focus bursts.

### Target architecture

- Split lightweight resource metadata, icons, and event-routing information
  from route and tab implementations.
- Dynamically load Monaco, Recharts, xterm, Prettier, and other heavy feature
  modules only when the corresponding route, tab, or editor is opened.
- Fetch Monaco extra libraries only after editor activation and reuse the
  browser cache for subsequent openings.
- Mount one OmniSearch instance and enable its queries only while the search is
  open.
- Treat WebSocket-delivered state as the primary freshness mechanism. Use slow
  60-second safety polling after a synchronized connection and retain the
  current faster intervals only while disconnected.
- On every initial connection and reconnection, invalidate the relevant
  WebSocket-managed query families and await a full refetch barrier before
  switching to the slow safety cadence. A detected event-sequence gap triggers
  the same barrier. Duplicate or out-of-order events must not move cached state
  backwards.
- Set query-specific `staleTime` and focus-refetch policies instead of relying
  on global defaults.
- Reuse list-item data in compact cards rather than starting additional full
  detail queries.

### Safety constraints

- Lazy loading must retain loading and error boundaries for every heavy route
  or tab.
- Disconnected, reconnecting, and background-tab behavior must remain correct;
  removing an interval must not make data permanently stale.
- A socket reporting `connected` is not sufficient proof of freshness. The UI
  may enter WebSocket-first mode only after its synchronization barrier
  completes successfully.
- Resource metadata used by WebSocket event routing must stay available without
  importing full resource implementations.
- Query keys and cache compatibility must remain stable unless a migration is
  included explicitly.

### Success criteria

- Monaco, Recharts, xterm, and Prettier are absent from the initial application
  chunks for a dashboard load.
- Initial JavaScript for a dashboard load is below 900 kB gzip, compared with
  the measured 1.923 MB gzip build baseline.
- Monaco typing requests are absent until an editor is opened.
- Before changing query policy, capture a real sixty-second browser-network
  baseline; the audit's 76/130/338 per-minute figures remain explicitly labeled
  static-cadence estimates. With WebSocket synchronized, an idle dashboard then
  performs at least 80% fewer background HTTP refetches than that runtime
  baseline.
- Opening search, an editor, terminal, or chart still loads and functions on
  demand; disconnect/reconnect tests prove polling fallback and recovery.

### Verification design

- Produce a Vite manifest and bundle-size comparison before and after each
  split.
- Record request counts for sixty seconds on a generic page and dashboard, with
  WebSocket connected and disconnected.
- Add component-level tests where available; otherwise document Playwright or
  browser verification for dynamic imports, loading states, reconnect, and
  focus behavior.
- Capture React Profiler data for OmniSearch and representative dashboard card
  counts to guard against moving network work into render work.
- Test a missed event, reconnect during a mutation, duplicate event,
  out-of-order event, sequence gap, failed synchronization refetch, and a
  prolonged background-tab suspension.

### Ordered PR checkpoints

1. Split resource metadata from implementations and lazy-load Monaco, charts,
   terminals, Prettier, and editor typings. This checkpoint is independent of
   Plan 2.
2. Mount one OmniSearch instance, gate closed-feature queries, reuse list-item
   data, and add query-specific stale/focus policies while retaining current
   fallback polling.
3. After Plan 2's additive event sequence contract merges, add synchronization
   barriers, sequence-gap handling, WebSocket-first cache updates, and the
   60-second safety cadence.

## Plan 4: Build, Release, and Dependency Efficiency

**Plan file:**
`docs/superpowers/plans/2026-07-10-komodo-build-release-dependency-performance.md`

### Outcome

Trusted CI and releases should reuse dependency and BuildKit work across runs
and across distinct release tags without weakening cache trust boundaries.

### P1 findings assigned to this plan

1. The tag-only release workflow writes GHA BuildKit caches that a different
   tag cannot restore.
2. `bin/binaries.Dockerfile` copies the full source before one Cargo build, and
   GHA cache export does not persist `RUN --mount=type=cache` contents by
   default.
3. The CI Cargo cache is commented out, so build and test jobs repeat dependency
   compilation.
4. `mogh_config` enables the default `cicada` feature although source search
   found no direct use; removing that edge reduced the simulated resolved graph
   from 729 to 664 package versions.

### Target architecture

- Publish stable GHCR registry cache references per build scope from trusted
  `main`; release tags may read those references but do not create tag-local
  cache namespaces. Untrusted PRs must never write to trusted cache references.
- Separate dependency recipe/build layers from frequently changing workspace
  source using `cargo-chef`. Keep cache mounts only as within-run acceleration;
  cross-run correctness must come from exported BuildKit layers.
- If cache mounts remain required, persist them explicitly rather than assuming
  the GHA layer exporter includes their contents.
- Restore compiler-aware Cargo caching in CI, save trusted default-branch
  entries separately from PR entries, and cancel superseded runs.
- Disable `mogh_config` default features only after real configuration fixtures
  and startup paths prove Cicada support is not required.

### Safety constraints

- Cache sources must not allow untrusted PR content to poison release builds.
- Release binaries must still be produced once and copied into the final image
  matrix without architecture drift.
- Cache changes must retain reproducibility from an empty cache.
- Dependency-feature removal must preserve supported config formats and startup
  behavior.

### Success criteria

- A second non-publishing rehearsal build restores the trusted binary and UI
  registry cache scopes produced by the first rehearsal, independent of a tag
  namespace.
- A source-only edit reuses dependency compilation layers while a dependency
  edit invalidates the correct layers.
- Across five cache-disabled and five warm-cache runs on the same runner class,
  warm CI median duration is at least 25% lower and cache restore/upload takes
  less than 20% of total job time.
- The resolved Cargo graph drops the verified Cicada-only closure and all
  configuration regression tests pass.
- Cold, cache-miss builds remain successful and pass the same binary version,
  startup, and image smoke tests as warm builds.

### Verification design

- Add a trusted `workflow_dispatch` rehearsal mode that never pushes product
  images or creates a release. Run it twice against a disposable registry-cache
  namespace, retain plain BuildKit logs, cache inventory, Cargo timings, and
  image sizes, then delete that namespace. Promote the same configuration to
  the stable cache reference only after review.
- Compare cold, warm, source-only-change, and lockfile-change builds.
- Track median CI duration over multiple runs instead of a single best run.
- Add configuration fixtures covering every supported loader path before
  changing `mogh_config` features.
- Treat checksums as informational unless all reproducibility inputs are pinned;
  functional equivalence is established by the declared smoke tests.

### Ordered PR checkpoints

1. Add non-publishing rehearsal mode, cache-hit metrics, and release-structure
   regression checks without changing production cache sources.
2. Add `cargo-chef` dependency layers and stable trusted registry-cache scopes;
   validate cold and two-run rehearsal paths before enabling them for releases.
3. Restore compiler-aware CI Cargo caching and superseded-run cancellation,
   then collect the five-run comparison.
4. Add configuration fixtures and disable the verified-unused Cicada feature
   edge in a separate dependency commit.

## Cross-Plan Dependencies and Execution Order

1. Plan 1 checkpoints 1–3 may execute independently. Merge Gate A is Plan 1
   checkpoint 4: it must expose one reviewed `PermissionSnapshotProvider`
   contract backed by `{ generation, mutation_in_progress }`, plus cross-Core
   compatibility tests and the cache kill switch.
2. Plan 2 checkpoints 1–3 may execute before Merge Gate A. Plan 2 checkpoint 4
   may begin only after `PermissionSnapshotProvider` merges; it must consume
   that provider rather than introducing another permission cache.
3. Plan 2 checkpoint 4 defines Merge Gate B: the existing Update event envelope
   remains valid and gains optional `{ stream_epoch, sequence }` metadata with
   the reset and comparison rules defined in Plan 2. Compatibility tests must
   cover Core restart, reconnection to another Core, gaps, duplicates,
   out-of-order delivery, and old/new Core plus old/new UI parsing before the
   gate merges.
4. Plan 3 checkpoints 1–2 may execute independently. Plan 3 checkpoint 3 may
   begin only after Merge Gate B and must implement the reconnect/refetch
   synchronization barrier before slowing fallback polling.
5. Plan 4 is independent and may proceed in parallel with any product-runtime
   checkpoint.

Plans 1 and 2 must not be implemented concurrently if both modify
`bin/core/src/permission.rs`, `bin/core/src/helpers/update.rs`, or the same
authorization tests. Plan 3 owns frontend resource loading and query policy;
Plan 2 owns backend event production and delivery authorization. Any necessary
edit to `ui/src/lib/socket.tsx` must be assigned explicitly before execution.

No umbrella plan is considered independently deployable as one atomic unit.
Each checkpoint must leave the repository releasable, and every later
checkpoint must list its merge-gate prerequisite explicitly.

## Implementation-Plan Requirements

Each of the four plan documents must:

- Follow the Superpowers `writing-plans` format with checkbox-sized tasks.
- Name exact files, functions, tests, commands, and expected results.
- Start behavior changes with a failing regression test, benchmark, trace, or
  reproducible baseline.
- Convert every qualitative target into a numeric budget in the measurement
  checkpoint before implementation begins. The plan must state the workload,
  runner/host class, sample count, percentile or median calculation, and the
  pass/fail threshold.
- Prefix every shell command with `rtk`.
- Separate measurement, minimal implementation, migration, verification, and
  rollback tasks.
- Include focused commit checkpoints with plain branch and commit names.
- Distinguish confirmed code facts from production assumptions that require
  live inspection.
- Avoid unrelated cleanup and avoid implementing P2/P3 findings opportunistically.

## Review and Rollback Strategy

Each checkpoint should produce small commits that can be reverted
independently. Database indexes require explicit foreground/background build
and removal steps. Permission caching ships with an authoritative-read kill
switch; a stuck mutation guard keeps the cache bypassed until recovery. A dirty
resource snapshot uses read-through and a five-second convergence SLO.

Update storage remains in the existing document and event payload changes are
additive, so rolling upgrades do not require dual-read storage. The 8 MiB cap is
an intentional data-retention policy and leaves a visible marker; reverting the
writer does not make already truncated output whole. Event coalescing and
frontend polling changes require the old/new compatibility matrix, reconnect
barrier, sequence-gap tests, and the 60-second safety poll before rollout.
Build-cache changes must always retain a verified empty-cache path and a
non-publishing rehearsal mode.

The program is complete only when all assigned P1 findings appear in exactly
one plan, every plan has measurable entry and exit criteria, and cross-plan file
ownership is unambiguous.
