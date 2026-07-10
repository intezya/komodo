# Komodo Runtime Backpressure and Update Events Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep Core and Periphery responsive during API-key bursts, large command and procedure output, batch fan-out, and Update WebSocket delivery by enforcing explicit CPU, task, process, queue, and persisted-log budgets.

**Architecture:** Move CPU-bound host work to bounded blocking tasks and publish immutable snapshots. Route every external and internal execution origin through one dispatcher, admit leaf, Action-orchestrator, Procedure-root, and monitor work through separate budgets, and execute nested Procedures with a bounded non-recursive tree scheduler. Capture command pipes while continuously draining them into bounded buffers, and replace whole-document procedure progress rewrites with 64 KiB delta appends flushed at most every 250 ms. Update events retain their existing JSON shape while gaining optional connection-local visible-stream metadata; one ordered fan-out hub authorizes each user once per event through Plan 1's generation-safe `PermissionSnapshotProvider` and then delivers the authorized event to that user's bounded connection queues.

**Tech Stack:** Rust 2024, Tokio, MongoDB 3.x, ArcSwap, sysinfo, bcrypt, Axum WebSockets, typeshare, existing Komodo Rust and TypeScript clients.

---

## Scope, dependency gates, and fixed budgets

This plan implements only the five P1 findings assigned to Plan 2 in
`docs/superpowers/specs/2026-07-10-komodo-p1-performance-program-design.md`.
It does not change bcrypt cost or credential semantics, introduce a new log
collection, tune the Rust release profile, or absorb P2/P3 cleanup.

Checkpoint 4 is blocked until Plan 1 exports this exact production contract:

```rust
// bin/core/src/permission/snapshot.rs
pub struct PermissionSnapshotProvider;

impl PermissionSnapshotProvider {
  pub async fn can_read_target(
    &self,
    user: &User,
    target: &ResourceTarget,
  ) -> anyhow::Result<bool>;
}

// Re-exported by bin/core/src/permission.rs
pub fn permission_snapshot_provider()
-> &'static PermissionSnapshotProvider;
```

`can_read_target` performs the two generation/guard reads defined by Plan 1
and falls back to authoritative reads whenever snapshots are disabled,
missing, or unsafe. The fan-out hub calls it exactly once for each distinct
user and event, so the provider's own double-read is the authorization
linearization point and no connection-local permission cache or extra
generation read is needed. Plan 2 consumes no provider-generation or metrics
API. Task 15 proves real database-read scaling from isolated, timestamp-scoped
Mongo profiler records instead.

Freeze these initial budgets in code before changing fan-out behavior. They are
deliberately internal constants for this P1; making them public configuration is
deferred until staging measurements show that operators need to tune them.

| Boundary | Active permits | Queue | Admission deadline | Full/timeout behavior |
|---|---:|---:|---:|---|
| Core bcrypt verification | 8 | 64 explicit verifier waiters | 2 s | reject the 65th waiter or a timed-out waiter without changing the bcrypt cost |
| Core user executions | 32 global, 4 per server/work key | 256 global, 16 per work key | 5 s | finalize an already-created Update with an `Execution admission` error and return HTTP 503 |
| Core child-awaiting orchestrators | 6 separate from leaf permits | 32 orchestrator and shared 256 global | 5 s | finalize the Update with `Orchestrator admission`; Action API calls and other children use normal leaf/orchestrator classification |
| Core Procedure roots | 8 separate from leaf permits | 32 Procedure and shared 256 global | 5 s | reject before creating the tree; nested nodes use the bounded tree scheduler rather than recursive permits |
| Procedure tree scheduler | 8 expansion workers, 32 leaf workers, 4,096 nodes per root | 256 ready jobs per root | root lifetime | fail the root with an explicit scheduler-overload terminal log; no worker waits for a descendant |
| Core monitoring | 16 global, 2 per server | 0 | 0 s | skip this cycle, emit `monitor_work_skipped`, retry on the next normal interval |
| Periphery user command processes | 6 | 64 | 5 s | return a failed `CommandOutput` with an explicit overload message |
| Periphery monitor command processes | 2 reserved | 0 | 0 s | omit that monitor sample and retry on the next poll |
| Procedure progress channel | 1 writer, 256 messages | 256 | producer backpressure | await capacity; cancellation returns an error to the procedure |
| Procedure database append | one writer per Update | 64 KiB batch | 250 ms flush | fail the procedure, drain/close the writer, and still persist terminal error state |
| Persisted Update log strings | 8 MiB total | none | none | append one deterministic marker, discard further log bytes, preserve terminal metadata |
| Captured command output | 8 MiB combined stdout/stderr | continuously drained | command-specific timeout | append explicit markers, kill/wait the child on timeout, preserve non-zero status |

Global and per-work-key leaf permits are acquired as a pair in **per-key then
global** order. Admission first reserves a per-work-key queue slot and a global
queue slot, then awaits the fair per-work-key semaphore and finally the fair
global semaphore under one deadline. A saturated key therefore never holds
global execution capacity, and no readiness probe lets a newer waiter jump an
older waiter. This order is the normative Plan 2 contract and must match the P1
program specification before implementation begins. Tokio owned permits make
cancellation and panic release automatic; one target can consume at most 16 of
the 256 queued positions.

## File map

**Checkpoint 1 — blocking work and lock scope**

- Modify `bin/core/src/auth/middleware.rs`: bounded `spawn_blocking` bcrypt verification and tests.
- Modify `bin/periphery/src/stats.rs`: blocking collector and immutable `StatsSnapshot` publisher.
- Modify `bin/periphery/src/state.rs`: replace `RwLock<StatsClient>` with `ArcSwap<StatsSnapshot>`.
- Modify `bin/periphery/src/api/poll.rs`: clone a snapshot before Docker/host awaits.
- Modify `bin/periphery/src/api/mod.rs`: serve process data from the immutable snapshot.

**Checkpoint 2 — client cadence and bounded output**

- Modify `client/core/rs/src/lib.rs`: shared 500 ms polling loop and additive timeout API.
- Modify `client/core/rs/src/request.rs`: private deadline-aware blocking read path.
- Modify `lib/command/src/lib.rs`: bounded concurrent stdout/stderr readers and deterministic timeout cleanup.
- Modify `lib/command/src/output.rs`: explicit timeout, truncation, and overload constructors.
- Create `lib/command/tests/bounded_output.rs`: 1/10/100 MiB, timeout, status, and RSS tests.
- Modify `Cargo.lock`: record the direct `libc` and later `thiserror` command-crate dependencies.
- Modify `bin/core/src/api/read/mod.rs`: one Core log-tail cap helper and tests.
- Modify `bin/core/src/api/read/{stack,swarm,deployment}.rs`: apply the cap before Periphery calls.
- Modify `bin/periphery/src/api/{compose,container/mod,swarm/service}.rs`: defense-in-depth tail caps and bounded command APIs.

**Checkpoint 3 — separate work budgets**

- Create `bin/core/src/runtime.rs`: leaf/Action/Procedure/monitor budgets, work-key resolution, instrumentation, and tests.
- Modify `bin/core/src/lib.rs`: register the runtime module and lag probe.
- Create `bin/core/src/api/execute/dispatch.rs`: the only root execution initialization, admission, spawn/await, and terminal-error path.
- Create `bin/core/src/helpers/procedure_tree.rs`: bounded coordinator plus expansion and leaf workers; no recursive parent-held worker.
- Modify `bin/core/src/api/execute/mod.rs`: delegate API and batch execution to the dispatcher without changing `ExecuteArgs`.
- Modify `bin/core/src/api/execute/action.rs` and
  `docsite/docs/automate/procedures.md`: reject Action-originated nested
  Action/Procedure orchestration before Update creation; leaf API calls remain
  supported.
- Modify `bin/core/src/api/execute/procedure.rs` and `bin/core/src/helpers/procedure.rs`: expose step/leaf resolution to the tree scheduler.
- Modify `bin/core/src/schedule.rs`, `bin/core/src/startup.rs`, `bin/core/src/api/listener/resources.rs`, and `bin/core/src/sync/deploy.rs`: remove direct root `.resolve(...)`, raw spawn fan-out, and `join_all`; use the dispatcher.
- Modify `bin/core/src/monitor/{mod,swarm}.rs`: non-blocking monitor admission and bounded streams.
- Create `lib/command/src/budget.rs`: independent Action-host, user, and monitor child-process budgets.
- Modify `lib/command/src/lib.rs`: acquire the correct command budget before spawning and expose process metrics.
- Modify `bin/periphery/src/docker/mod.rs`: route credential-fed `docker login`
  through the stdin-capable bounded user-command runner.
- Modify `bin/periphery/src/docker/compose.rs`: classify compose-project polling as monitor work.
- Modify `bin/periphery/src/api/container/mod.rs`: poll at most six bulk container commands concurrently.
- Create `bin/periphery/src/runtime_metrics.rs` and modify `bin/periphery/src/lib.rs`: Periphery lag, busy-time, stats-refresh, and command-budget windows.

**Checkpoint 4 — bounded Update/event pipeline**

- Modify `client/core/rs/src/entities/update.rs`: optional `stream_epoch` and `sequence`, plus compatibility tests.
- Regenerate `client/core/ts/src/types.ts`: optional TypeScript fields.
- Modify `bin/core/src/helpers/update.rs`: global 8 MiB Update log policy, delta append primitive, and event stamping.
- Create `bin/core/src/helpers/update_stream.rs`: single-writer progress buffer and 64 KiB/250 ms flush loop.
- Modify `bin/core/src/helpers/mod.rs`: register the stream helper.
- Modify `bin/core/src/helpers/procedure.rs`: emit lines through the progress writer.
- Modify `bin/core/src/api/execute/procedure.rs`: own, finish, and unwrap the progress writer before terminal state.
- Modify `bin/core/src/helpers/channel.rs`: ordered fan-out hub, connection registry, bounded event/connection queues, and authorization metrics.
- Modify `bin/core/src/api/ws/update.rs`: register the authenticated connection, consume its authorized queue, stamp sequence metadata, and handle lag/session invalidation.
- Create `bin/core/src/helpers/update_stream/staging.rs`: ignored staging fixture for exact 1/10/100 MiB progress-pipeline evidence.
- Create `scripts/performance/` and
  `scripts/performance/validate-runtime-backpressure.sh`: executable
  Core/Periphery/API/procedure/batch/WebSocket release gates.
- Create `docs/performance/` if absent.
- Create `docs/performance/runtime-backpressure-validation.md`: generated
  evidence table keyed by scenario and exact Update ID.
- Create `docs/performance/runtime-backpressure-validation.json`: raw
  machine-readable evidence consumed by the table.

## Ordered PR checkpoints

Use plain branch and PR names and target only `intezya/komodo`:

1. `runtime-blocking-work` — bcrypt, sysinfo, immutable snapshots, lock scope.
2. `runtime-bounded-output` — blocking client cadence, deadline API, pipe capture, tail caps, RSS tests.
3. `runtime-work-budgets` — unified root dispatch, Core
   leaf/Action/Procedure/monitor admission, bounded Procedure trees, and
   Periphery process budgets.
4. `runtime-update-events` — after Plan 1 checkpoint 4 merges; bounded progress writes, event sequence metadata, and shared snapshot authorization.

Do not stack checkpoint 4 on an unmerged Plan 1 branch. Rebase it onto the
fork's `main` containing `PermissionSnapshotProvider` so the dependency is
compiled and tested rather than mocked across PRs.

Checkpoint 3 also starts after Plan 1 checkpoint 3. It consumes the
dirty-aware `AllResourcesCache::read()` contract for work-key resolution and
the preloaded monitoring inventory; this avoids reintroducing raw stale-cache
reads or per-server relationship queries. This still precedes Merge Gate A,
which is Plan 1 checkpoint 4.

### Task 1: Offload and bound API-key bcrypt verification

**Files:**
- Modify: `bin/core/src/auth/middleware.rs:1-78`

- [ ] **Step 1: Write failing current-thread and concurrency tests**

Add this test module at the bottom of `bin/core/src/auth/middleware.rs`. The
first test proves the verifier does not run on the Tokio worker. The second
proves 32 simultaneous checks never run more than eight bcrypt closures.

```rust
#[cfg(test)]
mod tests {
  use std::{
    sync::{
      Arc,
      atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
  };

  use super::*;

  #[tokio::test(flavor = "current_thread")]
  async fn api_secret_verification_runs_off_runtime_thread() {
    let runtime_thread = thread::current().id();
    let verified = verify_api_secret_with(
      "secret".to_string(),
      "hash".to_string(),
      move |_, _| Ok(thread::current().id() != runtime_thread),
    )
    .await
    .unwrap();

    assert!(verified);
  }

  #[tokio::test]
  async fn api_secret_verification_never_exceeds_eight_workers() {
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let admission = Arc::new(BcryptAdmission::new(8, 64));
    let checks = (0..32).map(|_| {
      let active = active.clone();
      let peak = peak.clone();
      let admission = admission.clone();
      tokio::spawn(async move {
        verify_api_secret_with_admission(
          &admission,
          "secret".to_string(),
          "hash".to_string(),
          move |_, _| {
            let current =
              active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(current, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(20));
            active.fetch_sub(1, Ordering::SeqCst);
            Ok(true)
          },
        )
        .await
      })
    });

    for check in futures_util::future::join_all(checks).await {
      assert!(check.unwrap().unwrap());
    }
    assert!(peak.load(Ordering::SeqCst) <= 8);
  }

  async fn wait_for_queue(
    admission: &BcryptAdmission,
    available: usize,
  ) {
    tokio::time::timeout(Duration::from_secs(1), async {
      while admission.queued.available_permits() != available {
        tokio::task::yield_now().await;
      }
    })
    .await
    .unwrap();
  }

  async fn run_bcrypt_queue_capacity_case(
    active: usize,
    queued: usize,
  ) {
    let admission = Arc::new(BcryptAdmission::new(0, queued));
    let mut waiters = Vec::new();
    for _ in 0..queued {
      let admission = admission.clone();
      waiters.push(tokio::spawn(async move {
        verify_api_secret_with_admission(
          &admission,
          "secret".into(),
          "hash".into(),
          |_, _| Ok(true),
        )
        .await
      }));
    }
    wait_for_queue(&admission, 0).await;
    let error = verify_api_secret_with_admission(
      &admission,
      "secret".into(),
      "hash".into(),
      |_, _| Ok(true),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("queue full"));
    for waiter in waiters {
      waiter.abort();
      let _ = waiter.await;
    }
    wait_for_queue(&admission, queued).await;
    assert_eq!(active, BCRYPT_MAX_CONCURRENT);
  }

  async fn run_cancelled_bcrypt_waiter_case() {
    let admission = Arc::new(BcryptAdmission::new(0, 1));
    let waiter = {
      let admission = admission.clone();
      tokio::spawn(async move {
        verify_api_secret_with_admission(
          &admission,
          "secret".into(),
          "hash".into(),
          |_, _| Ok(true),
        )
        .await
      })
    };
    wait_for_queue(&admission, 0).await;
    waiter.abort();
    let _ = waiter.await;
    wait_for_queue(&admission, 1).await;
  }

  #[tokio::test]
  async fn api_secret_verifier_queue_rejects_waiter_sixty_five() {
    // Hold all eight workers, fill all 64 explicit waiter slots, then assert
    // one additional request receives the overload error without spawning.
    run_bcrypt_queue_capacity_case(8, 64).await;
  }

  #[tokio::test]
  async fn cancelled_bcrypt_waiter_releases_queue_slot() {
    run_cancelled_bcrypt_waiter_case().await;
  }
}
```

- [ ] **Step 2: Run the tests and verify RED**

Run: `rtk cargo test -p komodo_core auth::middleware::tests -- --nocapture`

Expected: compilation fails because `verify_api_secret_with` does not exist.

- [ ] **Step 3: Add the bounded blocking verifier**

Add these imports and helpers above `auth_api_key_get_user_id`:

```rust
use std::{sync::{Arc, LazyLock}, time::Duration};

use tokio::sync::Semaphore;

const BCRYPT_MAX_CONCURRENT: usize = 8;
const BCRYPT_MAX_QUEUED: usize = 64;
const BCRYPT_ADMISSION_TIMEOUT: Duration = Duration::from_secs(2);

struct BcryptAdmission {
  active: Arc<Semaphore>,
  queued: Arc<Semaphore>,
}

impl BcryptAdmission {
  fn new(active: usize, queued: usize) -> Self {
    Self {
      active: Arc::new(Semaphore::new(active)),
      queued: Arc::new(Semaphore::new(queued)),
    }
  }
}

static BCRYPT_ADMISSION: LazyLock<BcryptAdmission> =
  LazyLock::new(|| BcryptAdmission::new(
    BCRYPT_MAX_CONCURRENT,
    BCRYPT_MAX_QUEUED,
  ));

async fn verify_api_secret_with_admission<F>(
  admission: &BcryptAdmission,
  secret: String,
  hash: String,
  verify: F,
) -> anyhow::Result<bool>
where
  F: FnOnce(&str, &str) -> Result<bool, bcrypt::BcryptError>
    + Send
    + 'static,
{
  let queued = admission
    .queued
    .clone()
    .try_acquire_owned()
    .context("API key verifier queue full")?;
  let permit = tokio::time::timeout(
    BCRYPT_ADMISSION_TIMEOUT,
    admission.active.clone().acquire_owned(),
  )
  .await
  .context("API key verifier overloaded")?
  .context("API key verifier closed")?;
  drop(queued);

  let verified = tokio::task::spawn_blocking(move || {
    let _permit = permit;
    verify(&secret, &hash)
  })
  .await
  .context("API key verifier task failed")?
  .map_err(|_| anyhow!("Invalid user credentials"))?;

  Ok(verified)
}

async fn verify_api_secret_with<F>(
  secret: String,
  hash: String,
  verify: F,
) -> anyhow::Result<bool>
where
  F: FnOnce(&str, &str) -> Result<bool, bcrypt::BcryptError>
    + Send
    + 'static,
{
  verify_api_secret_with_admission(
    &BCRYPT_ADMISSION,
    secret,
    hash,
    verify,
  )
  .await
}

async fn verify_api_secret(
  secret: String,
  hash: String,
) -> anyhow::Result<bool> {
  verify_api_secret_with(secret, hash, bcrypt::verify).await
}
```

Replace the synchronous call in `auth_api_key_get_user_id` with owned inputs:

```rust
  if verify_api_secret(secret.to_string(), key.secret.clone()).await? {
    Ok(key.user_id)
  } else {
    Err(anyhow!("Invalid user credentials"))
  }
```

The queue permit exists only while waiting for an execution permit and is
released on cancellation, timeout, or admission. The execution permit is moved
into the blocking closure, so cancellation of the awaiting HTTP task cannot
admit more than eight still-running bcrypt jobs. The existing failed-auth rate
limiter remains defense in depth but is not counted as queue capacity because
successful bursts do not consume it.

- [ ] **Step 4: Verify GREEN and unchanged credential behavior**

Run: `rtk cargo test -p komodo_core auth::middleware::tests -- --nocapture`

Expected: both tests pass; invalid bcrypt hashes still surface as `Invalid user credentials`.

- [ ] **Step 5: Commit the bcrypt change**

```bash
rtk git add bin/core/src/auth/middleware.rs
rtk git commit -m "perf: bound api key verification"
```

### Task 2: Publish Periphery stats as immutable snapshots

**Files:**
- Modify: `bin/periphery/src/stats.rs:1-180`
- Modify: `bin/periphery/src/state.rs:164-175`
- Modify: `bin/periphery/src/api/poll.rs:16-45`
- Modify: `bin/periphery/src/api/mod.rs:208-219`

- [ ] **Step 1: Write the failing off-runtime collector test**

Append to `bin/periphery/src/stats.rs`:

```rust
#[cfg(test)]
mod tests {
  use std::thread;

  use super::*;

  #[tokio::test(flavor = "current_thread")]
  async fn stats_collection_runs_off_runtime_thread() {
    let runtime_thread = thread::current().id();
    let collected_off_runtime = run_collector_blocking(
      None::<()>,
      move |_| thread::current().id() != runtime_thread,
    )
    .await
    .unwrap()
    .1;

    assert!(collected_off_runtime);
  }
}
```

- [ ] **Step 2: Run the test and verify RED**

Run: `rtk cargo test -p komodo_periphery stats::tests::stats_collection_runs_off_runtime_thread`

Expected: compilation fails because `run_collector_blocking` does not exist.

- [ ] **Step 3: Split the mutable collector from the immutable snapshot**

Add this public snapshot and blocking helper in `bin/periphery/src/stats.rs`:

```rust
#[derive(Debug, Clone, Default)]
pub struct StatsSnapshot {
  pub stats: SystemStats,
  pub info: SystemInformation,
  pub processes: Vec<SystemProcess>,
}

async fn run_collector_blocking<T, R, F>(
  collector: Option<T>,
  collect: F,
) -> anyhow::Result<(T, R)>
where
  T: Default + Send + 'static,
  R: Send + 'static,
  F: FnOnce(&mut T) -> R + Send + 'static,
{
  tokio::task::spawn_blocking(move || {
    let mut collector = collector.unwrap_or_default();
    let result = collect(&mut collector);
    (collector, result)
  })
  .await
  .map_err(Into::into)
}

impl StatsClient {
  fn snapshot(&self) -> StatsSnapshot {
    StatsSnapshot {
      stats: self.get_system_stats(),
      info: self.info.clone(),
      processes: self.get_processes(),
    }
  }
}
```

Replace `spawn_polling_thread` with a loop that owns the collector and never
puts it behind an async lock:

```rust
pub fn spawn_polling_thread() {
  tokio::spawn(async move {
    let initialized = run_collector_blocking(
      None::<StatsClient>,
      |client| client.snapshot(),
    )
    .await;
    let (mut collector, initial) = match initialized {
      Ok(value) => value,
      Err(error) => {
        error!("Failed to initialize stats collector | {error:#}");
        return;
      }
    };
    stats_snapshot().store(Arc::new(initial));

    let polling_rate = periphery_config()
      .stats_polling_rate
      .to_string()
      .parse()
      .expect("invalid stats polling rate");

    loop {
      let ts = wait_until_timelength(polling_rate, 1).await as i64;
      match run_collector_blocking(Some(collector), move |client| {
        client.refresh();
        let mut snapshot = client.snapshot();
        snapshot.stats.refresh_ts = ts;
        snapshot
      })
      .await
      {
        Ok((next, snapshot)) => {
          collector = next;
          stats_snapshot().store(Arc::new(snapshot));
        }
        Err(error) => {
          error!("Stats collector task failed | {error:#}");
          return;
        }
      }
    }
  });
}
```

Import `std::sync::Arc` and `crate::state::stats_snapshot`; remove the mutation
of `client.stats` because the published `StatsSnapshot` now owns refresh
timestamps. Passing `None::<StatsClient>` is essential: it constructs
`StatsClient::default()` inside `spawn_blocking`, rather than eagerly on the
Tokio worker before the helper is called.

- [ ] **Step 4: Replace the global RwLock with ArcSwap**

In `bin/periphery/src/state.rs`, replace `stats_client` with:

```rust
pub fn stats_snapshot() -> &'static ArcSwap<StatsSnapshot> {
  static STATS_SNAPSHOT: OnceLock<ArcSwap<StatsSnapshot>> =
    OnceLock::new();
  STATS_SNAPSHOT.get_or_init(Default::default)
}
```

Import `crate::stats::StatsSnapshot`. Remove the unused `RwLock` import only if
no other state value uses it.

- [ ] **Step 5: Drop snapshot ownership before every network await**

In `bin/periphery/src/api/poll.rs`, replace the read guard with:

```rust
    let snapshot = stats_snapshot().load_full();
    let system_info = snapshot.info.clone();
    let system_stats = self.include_stats.then(|| snapshot.stats.clone());
    drop(snapshot);

    let docker = if self.include_docker {
      let client = docker_client().load();
      if let Some(client) = client.iter().next() {
        Some(docker_lists(client).await)
      } else {
        None
      }
    } else {
      None
    };

    Ok(PollStatusResponse {
      periphery_info: periphery_information().await,
      system_info,
      system_stats,
      docker,
    })
```

In `bin/periphery/src/api/mod.rs`, return the cached processes:

```rust
    Ok(stats_snapshot().load().processes.clone())
```

- [ ] **Step 6: Verify stats and PollStatus**

Run: `rtk cargo test -p komodo_periphery stats::tests -- --nocapture`

Expected: the collector test passes.

Run: `rtk cargo test -p komodo_periphery`

Expected: all Periphery tests pass without a `RwLock<StatsClient>` reference.

- [ ] **Step 7: Commit immutable stats snapshots**

```bash
rtk git add bin/periphery/src/stats.rs bin/periphery/src/state.rs bin/periphery/src/api/poll.rs bin/periphery/src/api/mod.rs
rtk git commit -m "perf: publish immutable periphery stats"
```

### Task 3: Close checkpoint 1 with a runtime regression pass

**Files:**
- Verify only

- [ ] **Step 1: Run focused formatting and tests**

Run: `rtk cargo fmt --all -- --check`

Expected: exit 0.

Run: `rtk cargo test -p komodo_core auth::middleware::tests`

Expected: 2 tests pass.

Run: `rtk cargo test -p komodo_periphery`

Expected: all Periphery tests pass.

- [ ] **Step 2: Build both binaries**

Run: `rtk cargo build -p komodo_core -p komodo_periphery`

Expected: both binaries compile with no lock held across the PollStatus Docker or host-information awaits.

- [ ] **Step 3: Push and open the fork-only checkpoint PR**

```bash
rtk git push -u origin runtime-blocking-work
rtk gh pr create --repo intezya/komodo --base main --head runtime-blocking-work --title "Bound runtime blocking work" --body "Moves API-key bcrypt and Periphery sysinfo collection off Tokio workers, publishes immutable stats snapshots, and shortens PollStatus lock scope. Verification: cargo test -p komodo_core auth::middleware::tests; cargo test -p komodo_periphery; cargo build -p komodo_core -p komodo_periphery."
```

Expected: the PR base repository is `intezya/komodo` and CI is green before merge.

### Task 4: Add 500 ms blocking-client cadence and an additive deadline API

**Files:**
- Modify: `client/core/rs/src/lib.rs:1-214`
- Modify: `client/core/rs/src/request.rs:90-260`

- [ ] **Step 1: Write failing deterministic polling tests**

Append these feature-gated tests to `client/core/rs/src/lib.rs`:

```rust
#[cfg(all(test, feature = "blocking"))]
mod blocking_poll_tests {
  use std::{cell::Cell, time::Duration};

  use super::*;
  use entities::update::{Update, UpdateStatus};

  #[test]
  fn blocking_poll_sleeps_five_hundred_ms_between_reads() {
    let reads = Cell::new(0);
    let mut sleeps = Vec::new();
    let update = poll_update_blocking_with(
      "update-1".to_string(),
      None,
      |_| {
        let current = reads.get() + 1;
        reads.set(current);
        let mut update = Update::default();
        update.status = if current == 3 {
          UpdateStatus::Complete
        } else {
          UpdateStatus::InProgress
        };
        Ok(update)
      },
      |duration| sleeps.push(duration),
      || Duration::ZERO,
    )
    .unwrap();

    assert_eq!(update.status, UpdateStatus::Complete);
    assert_eq!(reads.get(), 3);
    assert_eq!(
      sleeps,
      [Duration::from_millis(500), Duration::from_millis(500)]
    );
  }

  #[test]
  fn blocking_poll_timeout_stops_at_declared_deadline() {
    let elapsed = Cell::new(Duration::ZERO);
    let error = poll_update_blocking_with(
      "update-2".to_string(),
      Some(Duration::from_secs(1)),
      |_| {
        let mut update = Update::default();
        update.status = UpdateStatus::InProgress;
        Ok(update)
      },
      |duration| elapsed.set(elapsed.get() + duration),
      || elapsed.get(),
    )
    .unwrap_err();

    assert!(error.to_string().contains("update-2"));
    assert!(error.to_string().contains("1s"));
    assert_eq!(elapsed.get(), Duration::from_secs(1));
  }
}
```

- [ ] **Step 2: Run the tests and verify RED**

Run: `rtk cargo test -p komodo_client --features blocking blocking_poll_tests`

Expected: compilation fails because `poll_update_blocking_with` is missing.

- [ ] **Step 3: Implement one shared blocking poll loop**

Add this helper near the existing client poll methods:

```rust
#[cfg(feature = "blocking")]
fn poll_update_blocking_with<Get, Sleep, Elapsed>(
  update_id: String,
  timeout: Option<Duration>,
  mut get: Get,
  mut sleep: Sleep,
  elapsed: Elapsed,
) -> anyhow::Result<entities::update::Update>
where
  Get: FnMut(&str) -> anyhow::Result<entities::update::Update>,
  Sleep: FnMut(Duration),
  Elapsed: Fn() -> Duration,
{
  const POLL_INTERVAL: Duration = Duration::from_millis(500);

  loop {
    if let Some(timeout) = timeout
      && elapsed() >= timeout
    {
      return Err(anyhow::anyhow!(
        "timed out waiting {timeout:?} for update {update_id}"
      ));
    }

    let update = get(&update_id)?;
    if update.status == entities::update::UpdateStatus::Complete {
      return Ok(update);
    }

    let sleep_for = timeout
      .map(|timeout| POLL_INTERVAL.min(timeout.saturating_sub(elapsed())))
      .unwrap_or(POLL_INTERVAL);
    sleep(sleep_for);
  }
}
```

Replace the existing blocking method and add the new additive API:

```rust
  #[cfg(feature = "blocking")]
  pub fn poll_update_until_complete(
    &self,
    update_id: impl Into<String>,
  ) -> anyhow::Result<entities::update::Update> {
    let started = std::time::Instant::now();
    poll_update_blocking_with(
      update_id.into(),
      None,
      |id| self.read(api::read::GetUpdate { id: id.to_string() }),
      std::thread::sleep,
      || started.elapsed(),
    )
  }

  #[cfg(feature = "blocking")]
  pub fn poll_update_until_complete_with_timeout(
    &self,
    update_id: impl Into<String>,
    timeout: Duration,
  ) -> anyhow::Result<entities::update::Update> {
    let started = std::time::Instant::now();
    poll_update_blocking_with(
      update_id.into(),
      Some(timeout),
      |id| {
        self.read_with_timeout(
          api::read::GetUpdate { id: id.to_string() },
          timeout.saturating_sub(started.elapsed()),
        )
      },
      std::thread::sleep,
      || started.elapsed(),
    )
  }
```

The original method retains its return type and unlimited total wait, but now
performs no more than two polls per second.

- [ ] **Step 4: Give each blocking HTTP read the remaining deadline**

In `client/core/rs/src/request.rs`, add this private method:

```rust
  #[cfg(feature = "blocking")]
  pub(crate) fn read_with_timeout<T>(
    &self,
    request: T,
    timeout: std::time::Duration,
  ) -> anyhow::Result<T::Response>
  where
    T: Serialize + KomodoReadRequest,
    T::Response: DeserializeOwned,
  {
    self.post_with_timeout(
      "/read",
      json!({
        "type": T::req_type(),
        "params": request
      }),
      timeout,
    )
  }
```

Make the existing blocking `post` delegate to a new request-builder timeout
path:

```rust
  #[cfg(feature = "blocking")]
  fn post<B: Serialize + std::fmt::Debug, R: DeserializeOwned>(
    &self,
    endpoint: &str,
    body: B,
  ) -> anyhow::Result<R> {
    self.post_inner(endpoint, body, None)
  }

  #[cfg(feature = "blocking")]
  fn post_with_timeout<
    B: Serialize + std::fmt::Debug,
    R: DeserializeOwned,
  >(
    &self,
    endpoint: &str,
    body: B,
    timeout: std::time::Duration,
  ) -> anyhow::Result<R> {
    self.post_inner(endpoint, body, Some(timeout))
  }

  #[cfg(feature = "blocking")]
  fn post_inner<
    B: Serialize + std::fmt::Debug,
    R: DeserializeOwned,
  >(
    &self,
    endpoint: &str,
    body: B,
    timeout: Option<std::time::Duration>,
  ) -> anyhow::Result<R> {
    let mut req = self
      .reqwest
      .post(format!("{}{endpoint}", self.address))
      .header("x-api-key", &self.key)
      .header("x-api-secret", &self.secret)
      .header("content-type", "application/json")
      .json(&body);
    if let Some(timeout) = timeout {
      req = req.timeout(timeout);
    }
    let res = req.send().context("failed to reach Komodo API")?;
    let status = res.status();
    if status.is_success() {
      res.json().map_err(|error| anyhow!("{error:#?}").context(status))
    } else {
      match res.text() {
        Ok(res) => Err(deserialize_error(res).context(status)),
        Err(error) => Err(anyhow!("{error:?}").context(status)),
      }
    }
  }
```

- [ ] **Step 5: Verify both client feature modes**

Run: `rtk cargo test -p komodo_client --features blocking blocking_poll_tests`

Expected: 2 tests pass.

Run: `rtk cargo check -p komodo_client`

Expected: the async client still compiles unchanged.

Run: `rtk cargo check -p komodo_client --features blocking`

Expected: the blocking client compiles with both poll methods.

- [ ] **Step 6: Commit the client cadence and deadline API**

```bash
rtk git add client/core/rs/src/lib.rs client/core/rs/src/request.rs
rtk git commit -m "fix: bound blocking update polling"
```

### Task 5: Replace Command::output with bounded pipe draining

**Files:**
- Modify: `Cargo.toml` — add the workspace `libc` dependency.
- Modify: `lib/command/Cargo.toml` — consume `libc.workspace = true`.
- Modify: `lib/command/src/lib.rs:1-340`
- Modify: `lib/command/src/output.rs:1-50`
- Create: `lib/command/tests/bounded_output.rs`

- [ ] **Step 1: Write failing bounded-output integration tests**

Create `lib/command/tests/bounded_output.rs`:

```rust
use std::time::{Duration, Instant};

use command::{
  COMMAND_OUTPUT_LIMIT_BYTES, OUTPUT_TRUNCATION_MARKER,
  run_shell_command, run_shell_command_with_timeout,
};

#[tokio::test]
async fn captures_at_most_eight_mib_and_marks_truncation() {
  let output = run_shell_command(
    "yes x | head -c 10485760",
    None,
  )
  .await;

  assert!(output.success());
  assert!(output.stdout.contains(OUTPUT_TRUNCATION_MARKER));
  assert!(
    output.stdout.len() + output.stderr.len()
      <= COMMAND_OUTPUT_LIMIT_BYTES
  );
}

#[tokio::test]
async fn timeout_kills_waits_and_returns_without_pipe_hang() {
  let started = Instant::now();
  let output = run_shell_command_with_timeout(
    "printf before-timeout; sleep 30",
    None,
    Duration::from_millis(100),
  )
  .await;

  assert!(!output.success());
  assert!(output.stdout.contains("before-timeout"));
  assert!(output.stderr.contains("Command timed out"));
  assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn truncation_does_not_hide_command_failure() {
  let output = run_shell_command(
    "yes error | head -c 10485760 >&2; exit 17",
    None,
  )
  .await;

  assert!(!output.success());
  assert!(output.stderr.contains(OUTPUT_TRUNCATION_MARKER));
}

#[tokio::test]
async fn stdout_and_stderr_share_one_eight_mib_budget() {
  let output = run_shell_command(
    "(yes out | head -c 6291456) & (yes err | head -c 6291456 >&2) & wait",
    None,
  )
  .await;

  assert!(output.success());
  assert!(
    output.stdout.len() + output.stderr.len()
      <= COMMAND_OUTPUT_LIMIT_BYTES
  );
  assert!(
    output.stdout.contains(OUTPUT_TRUNCATION_MARKER)
      || output.stderr.contains(OUTPUT_TRUNCATION_MARKER)
  );
}

#[tokio::test]
async fn invalid_utf8_expansion_is_marked_and_bounded() {
  let output = run_shell_command(
    "i=0; while [ $i -lt 4000000 ]; do printf '\\377'; i=$((i+1)); done",
    None,
  )
  .await;
  assert!(output.stdout.contains(OUTPUT_TRUNCATION_MARKER));
  assert!(
    output.stdout.len() + output.stderr.len()
      <= COMMAND_OUTPUT_LIMIT_BYTES
  );
}

fn process_is_running(pid: &str) -> bool {
  let output = std::process::Command::new("ps")
    .args(["-o", "stat=", "-p", pid])
    .output()
    .unwrap();
  output.status.success()
    && !String::from_utf8_lossy(&output.stdout).trim_start().starts_with('Z')
}

async fn assert_process_stops(pid: &str) {
  let deadline = Instant::now() + Duration::from_secs(2);
  while process_is_running(pid) && Instant::now() < deadline {
    tokio::time::sleep(Duration::from_millis(20)).await;
  }
  assert!(!process_is_running(pid), "descendant remained live");
}

#[tokio::test]
async fn timeout_kills_background_descendant_and_closes_pipes() {
  let output = run_shell_command_with_timeout(
    "sleep 30 & child=$!; printf '%s\\n' \"$child\"; wait",
    None,
    Duration::from_millis(100),
  )
  .await;
  let pid = output.stdout.lines().next().unwrap();
  assert_process_stops(pid).await;
}

#[tokio::test]
async fn successful_parent_cannot_leave_a_live_pipe_owner() {
  let output = run_shell_command(
    "sleep 30 & child=$!; printf '%s\\n' \"$child\"",
    None,
  )
  .await;
  let pid = output.stdout.lines().next().unwrap();
  assert!(!output.success());
  assert!(output.stderr.contains("descendant cleanup"));
  assert!(
    output.stdout.len() + output.stderr.len()
      <= COMMAND_OUTPUT_LIMIT_BYTES
  );
  assert_process_stops(pid).await;
}

#[tokio::test]
async fn cancelling_command_future_kills_background_descendant() {
  let pid_file = std::env::temp_dir().join(format!(
    "komodo-command-cancel-{}",
    std::process::id(),
  ));
  let _ = std::fs::remove_file(&pid_file);
  let command = format!(
    "sleep 30 & child=$!; printf '%s' \"$child\" > '{}'; wait",
    pid_file.display(),
  );
  let task = tokio::spawn(async move {
    run_shell_command(&command, None).await
  });
  let deadline = Instant::now() + Duration::from_secs(1);
  while !pid_file.exists() && Instant::now() < deadline {
    tokio::time::sleep(Duration::from_millis(10)).await;
  }
  let pid = std::fs::read_to_string(&pid_file).unwrap();
  task.abort();
  let _ = task.await;
  assert_process_stops(pid.trim()).await;
  let _ = std::fs::remove_file(pid_file);
}
```

- [ ] **Step 2: Run tests and verify RED**

Run: `rtk cargo test -p command --test bounded_output -- --nocapture`

Expected: compilation fails because the output limit and marker are missing.

- [ ] **Step 3: Add explicit CommandOutput constructors**

In `lib/command/src/output.rs`, retain `from`, `from_err`, and `success`, and
add:

```rust
impl CommandOutput {
  pub fn from_parts(
    status: ExitStatus,
    stdout: String,
    stderr: String,
  ) -> Self {
    Self { status, stdout, stderr }
  }

  pub fn append_timeout(&mut self, timeout: Duration) {
    if !self.stderr.is_empty() {
      self.stderr.push('\n');
    }
    self.stderr.push_str(&format!(
      "Command timed out after {}ms",
      timeout.as_millis()
    ));
    self.status = ExitStatus::from_raw(1 << 8);
  }

  pub fn append_cleanup_error(&mut self, message: &str) {
    if !self.stderr.is_empty() {
      self.stderr.push('\n');
    }
    self.stderr.push_str(message);
    self.status = ExitStatus::from_raw(1 << 8);
  }

  pub fn from_overload(message: &str) -> Self {
    Self {
      status: ExitStatus::from_raw(1 << 8),
      stdout: String::new(),
      stderr: message.to_string(),
    }
  }
}
```

Import `std::time::Duration`; keep the existing Unix `ExitStatusExt` import
required by `ExitStatus::from_raw`. Add an owned `SecretInput(Vec<u8>)` whose
`Drop` fills the buffer with zeroes, plus an stdin-capable public runner. It
sets `Stdio::piped`, writes the owned bytes with `AsyncWriteExt::write_all`,
zeroes/drops them, closes stdin, and then uses the exact same concurrent bounded
stdout/stderr drains, process-group guard, timeout, and cleanup path as every
other command. No input bytes enter `CommandOutput`, tracing, or error text.
Add `stdin_command_is_bounded_and_does_not_echo_secret`, with a child that
reads stdin and emits 10 MiB of unrelated output; assert the 8 MiB cap/marker
and absence of the secret. Task 10 routes `docker login` through this runner
and the user-process budget; no direct `wait_with_output` exception remains.

- [ ] **Step 4: Implement bounded concurrent readers and deterministic timeout cleanup**

Add `libc = "0.2"` under `[workspace.dependencies]` in the root
`Cargo.toml`, then add `libc.workspace = true` under `[dependencies]` in
`lib/command/Cargo.toml`. This dependency is used only for the Unix
process-group `kill(2)` call below.

Add these definitions to `lib/command/src/lib.rs`:

```rust
use std::{
  os::unix::process::{CommandExt, ExitStatusExt},
  process::ExitStatus,
  sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
  },
};

use tokio::io::{AsyncRead, AsyncReadExt};

pub const COMMAND_OUTPUT_LIMIT_BYTES: usize = 8 * 1024 * 1024;
pub const OUTPUT_TRUNCATION_MARKER: &str =
  "\n[komodo: command output truncated at 8 MiB]\n";
const PIPE_READ_BYTES: usize = 16 * 1024;
const PIPE_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

struct CapturedPipe {
  bytes: Vec<u8>,
  truncated: bool,
}

async fn drain_pipe(
  mut reader: impl AsyncRead + Unpin,
  retained: Arc<AtomicUsize>,
) -> std::io::Result<CapturedPipe> {
  let mut bytes = Vec::with_capacity(PIPE_READ_BYTES);
  let mut chunk = [0_u8; PIPE_READ_BYTES];
  let mut truncated = false;
  loop {
    let read = reader.read(&mut chunk).await?;
    if read == 0 {
      break;
    }
    let keep = loop {
      let current = retained.load(Ordering::Acquire);
      let available =
        COMMAND_OUTPUT_LIMIT_BYTES.saturating_sub(current);
      let keep = read.min(available);
      if retained
        .compare_exchange(
          current,
          current + keep,
          Ordering::AcqRel,
          Ordering::Acquire,
        )
        .is_ok()
      {
        break keep;
      }
    };
    bytes.extend_from_slice(&chunk[..keep]);
    truncated |= keep < read;
  }
  Ok(CapturedPipe { bytes, truncated })
}

fn lossy_utf8_len(input: &[u8]) -> usize {
  input.utf8_chunks().fold(0, |length, chunk| {
    length
      .saturating_add(chunk.valid().len())
      .saturating_add(
        (!chunk.invalid().is_empty() as usize)
          * char::REPLACEMENT_CHARACTER.len_utf8(),
      )
  })
}

fn lossy_prefix(input: &[u8], max: usize) -> (String, bool) {
  let mut output = String::with_capacity(max.min(input.len()));
  let mut truncated = false;
  for chunk in input.utf8_chunks() {
    let valid = chunk.valid();
    let remaining = max.saturating_sub(output.len());
    if valid.len() > remaining {
      let mut end = remaining;
      while !valid.is_char_boundary(end) {
        end -= 1;
      }
      output.push_str(&valid[..end]);
      truncated = true;
      break;
    }
    output.push_str(valid);
    if !chunk.invalid().is_empty() {
      if output.len() + char::REPLACEMENT_CHARACTER.len_utf8()
        > max
      {
        truncated = true;
        break;
      }
      output.push(char::REPLACEMENT_CHARACTER);
    }
  }
  (output, truncated)
}

fn finish_pipes(
  stdout: CapturedPipe,
  stderr: CapturedPipe,
  budget: usize,
) -> (String, String) {
  let marker_bytes = OUTPUT_TRUNCATION_MARKER.len();
  let stdout_lossy = lossy_utf8_len(&stdout.bytes);
  let stderr_lossy = lossy_utf8_len(&stderr.bytes);
  let total_bytes = stdout_lossy.saturating_add(stderr_lossy);
  let must_truncate =
    stdout.truncated || stderr.truncated || total_bytes > budget;
  let data_budget = if must_truncate {
    budget.saturating_sub(marker_bytes)
  } else {
    budget
  };
  let (mut stdout_output, stdout_cut) =
    lossy_prefix(&stdout.bytes, data_budget);
  let stderr_budget =
    data_budget.saturating_sub(stdout_output.len());
  let (mut stderr_output, stderr_cut) =
    lossy_prefix(&stderr.bytes, stderr_budget);
  if must_truncate && budget >= marker_bytes {
    if stdout.truncated || stdout_cut {
      stdout_output.push_str(OUTPUT_TRUNCATION_MARKER);
    } else if stderr.truncated || stderr_cut {
      stderr_output.push_str(OUTPUT_TRUNCATION_MARKER);
    } else {
      stderr_output.push_str(OUTPUT_TRUNCATION_MARKER);
    }
  }
  (stdout_output, stderr_output)
}

fn empty_truncated_pipe() -> CapturedPipe {
  CapturedPipe { bytes: Vec::new(), truncated: true }
}

struct PipeDrainResult {
  stdout: CapturedPipe,
  stderr: CapturedPipe,
  deadline_elapsed: bool,
}

async fn await_pipes(
  mut stdout: tokio::task::JoinHandle<std::io::Result<CapturedPipe>>,
  mut stderr: tokio::task::JoinHandle<std::io::Result<CapturedPipe>>,
) -> PipeDrainResult {
  let deadline = tokio::time::Instant::now() + PIPE_DRAIN_TIMEOUT;
  let (stdout_result, stderr_result) = tokio::join!(
    tokio::time::timeout_at(deadline, &mut stdout),
    tokio::time::timeout_at(deadline, &mut stderr),
  );
  let mut deadline_elapsed = false;
  let stdout = match stdout_result {
    Ok(Ok(Ok(pipe))) => pipe,
    Ok(Ok(Err(_))) | Ok(Err(_)) => empty_truncated_pipe(),
    Err(_) => {
      deadline_elapsed = true;
      stdout.abort();
      let _ = stdout.await;
      empty_truncated_pipe()
    }
  };
  let stderr = match stderr_result {
    Ok(Ok(Ok(pipe))) => pipe,
    Ok(Ok(Err(_))) | Ok(Err(_)) => empty_truncated_pipe(),
    Err(_) => {
      deadline_elapsed = true;
      stderr.abort();
      let _ = stderr.await;
      empty_truncated_pipe()
    }
  };
  PipeDrainResult { stdout, stderr, deadline_elapsed }
}

fn kill_process_group(pgid: i32) {
  // SAFETY: `pgid` was captured from the child created with process_group(0).
  let _ = unsafe { libc::kill(-pgid, libc::SIGKILL) };
}

struct ProcessGroupGuard {
  pgid: i32,
  armed: bool,
}

impl ProcessGroupGuard {
  fn new(pgid: i32) -> Self {
    Self { pgid, armed: true }
  }

  fn disarm(&mut self) {
    self.armed = false;
  }
}

impl Drop for ProcessGroupGuard {
  fn drop(&mut self) {
    if self.armed {
      kill_process_group(self.pgid);
    }
  }
}

async fn kill_process_group_and_wait(
  child: &mut tokio::process::Child,
  pgid: i32,
) -> ExitStatus {
  kill_process_group(pgid);
  let _ = child.start_kill();
  child
    .wait()
    .await
    .unwrap_or_else(|_| ExitStatus::from_raw(1 << 8))
}
```

Replace `run_command_output` with a spawn/wait/drain implementation:

```rust
async fn run_command_output(
  mut cmd: Command,
  timeout: Option<Duration>,
) -> CommandOutput {
  cmd
    .kill_on_drop(true)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
  cmd.as_std_mut().process_group(0);

  let mut child = match cmd.spawn() {
    Ok(child) => child,
    Err(error) => return CommandOutput::from_err(error),
  };
  let pgid = child.id().expect("spawned child has pid") as i32;
  // Declared after the Child so cancellation drops this guard first.
  let mut process_group = ProcessGroupGuard::new(pgid);
  let stdout = child.stdout.take().expect("stdout configured as piped");
  let stderr = child.stderr.take().expect("stderr configured as piped");
  let retained = Arc::new(AtomicUsize::new(0));
  let stdout = tokio::spawn(drain_pipe(stdout, retained.clone()));
  let stderr = tokio::spawn(drain_pipe(stderr, retained));

  let (status, timed_out) = match timeout {
    Some(timeout) => match tokio::time::timeout(timeout, child.wait()).await {
      Ok(Ok(status)) => (status, None),
      Ok(Err(error)) => {
        let _ = kill_process_group_and_wait(&mut child, pgid).await;
        let _ = await_pipes(stdout, stderr).await;
        process_group.disarm();
        return CommandOutput::from_err(error);
      }
      Err(_) => {
        let status =
          kill_process_group_and_wait(&mut child, pgid).await;
        (status, Some(timeout))
      }
    },
    None => match child.wait().await {
      Ok(status) => (status, None),
      Err(error) => {
        let _ = kill_process_group_and_wait(&mut child, pgid).await;
        let _ = await_pipes(stdout, stderr).await;
        process_group.disarm();
        return CommandOutput::from_err(error);
      }
    },
  };

  let PipeDrainResult {
    stdout,
    stderr,
    deadline_elapsed,
  } = await_pipes(stdout, stderr).await;
  if deadline_elapsed {
    // The direct child may have exited successfully while a descendant still
    // owns stdout/stderr. Kill the stored group before releasing its permit.
    kill_process_group(pgid);
  }
  let timeout_message = timed_out.map(|timeout| {
    format!(
      "Command timed out after {}ms",
      timeout.as_millis(),
    )
  });
  const CLEANUP_MESSAGE: &str =
    "Command descendant cleanup exceeded the pipe drain deadline";
  let cleanup_message = deadline_elapsed.then_some(CLEANUP_MESSAGE);
  // Reserve each possible message plus its worst-case newline delimiter.
  // Timeout and successful-parent cleanup may occur together.
  let diagnostic_reserve = timeout_message
    .as_ref()
    .map(|message| message.len() + 1)
    .unwrap_or_default()
    .saturating_add(
      cleanup_message
        .map(|message| message.len() + 1)
        .unwrap_or_default(),
    )
    .min(COMMAND_OUTPUT_LIMIT_BYTES);
  let pipe_budget =
    COMMAND_OUTPUT_LIMIT_BYTES.saturating_sub(diagnostic_reserve);
  let (stdout, stderr) =
    finish_pipes(stdout, stderr, pipe_budget);
  let mut output = CommandOutput::from_parts(status, stdout, stderr);
  if let Some(timeout) = timed_out {
    output.append_timeout(timeout);
  }
  if deadline_elapsed {
    output.append_cleanup_error(CLEANUP_MESSAGE);
  }
  debug_assert!(
    output.stdout.len() + output.stderr.len()
      <= COMMAND_OUTPUT_LIMIT_BYTES
  );
  process_group.disarm();
  output
}
```

Reserving the exact worst-case bytes for both diagnostics before finishing
either pipe keeps `stdout.len() + stderr.len()` within 8 MiB when timeout and
descendant cleanup coexist. `lossy_prefix`
walks `utf8_chunks` without ever allocating a fully expanded lossy string, so
four million invalid bytes cannot transiently become twelve million retained
bytes. The shared atomic quota prevents the two concurrent readers from
retaining 8 MiB each. Both readers share one drain deadline and timed-out tasks
are aborted **and joined**. Timeout and wait-error paths kill the child's whole
Unix process group and reap the direct child. The process-group id is captured
before `wait`; if a successful direct child leaves a descendant holding either
pipe, the shared drain deadline kills that stored group, marks the output
failed with a deterministic descendant-cleanup message, and only then releases
the command permit. Tests poll process state and treat zombies as stopped
instead of relying on a racy immediate `kill -0`.

Remove the earlier `.stdout(Stdio::piped()).stderr(Stdio::piped())` setup from
`run_standard_command_inner`; the shared runner now configures both standard
and shell commands identically.

- [ ] **Step 5: Verify caps, draining, timeout, and exit status**

Run: `rtk cargo test -p command --test bounded_output -- --nocapture`

Expected: 8 tests pass; the 10 MiB producers are fully drained, stdout and
stderr share one 8 MiB retained budget, timeout returns in under 2 seconds,
invalid UTF-8 expansion stays bounded, timeout/success/cancellation leave no
live background descendants, and exit 17 remains a failure.

Run: `rtk cargo test -p command`

Expected: all existing command timeout tests also pass.

- [ ] **Step 6: Commit bounded pipe capture**

```bash
rtk git add Cargo.toml Cargo.lock lib/command/Cargo.toml lib/command/src/lib.rs lib/command/src/output.rs lib/command/tests/bounded_output.rs
rtk git commit -m "perf: bound command output capture"
```

### Task 6: Enforce line caps on every log route

**Files:**
- Modify: `bin/core/src/api/read/mod.rs`
- Modify: `bin/core/src/api/read/stack.rs:160-224`
- Modify: `bin/core/src/api/read/swarm.rs:250-278`
- Modify: `bin/core/src/api/read/deployment.rs:130-178`
- Modify: `bin/periphery/src/api/compose.rs:38-72`
- Modify: `bin/periphery/src/api/container/mod.rs:45-78`
- Modify: `bin/periphery/src/api/swarm/service.rs:50-94`

- [ ] **Step 1: Write the failing Core cap test**

Add to the existing test module in `bin/core/src/api/read/mod.rs`, or create one
at the end if absent:

```rust
#[cfg(test)]
mod log_limit_tests {
  use super::*;

  #[test]
  fn log_tail_is_capped_at_five_thousand_lines() {
    assert_eq!(cap_log_tail(50), 50);
    assert_eq!(cap_log_tail(5_000), 5_000);
    assert_eq!(cap_log_tail(u64::MAX), 5_000);
  }
}
```

- [ ] **Step 2: Run the test and verify RED**

Run: `rtk cargo test -p komodo_core api::read::log_limit_tests`

Expected: compilation fails because `cap_log_tail` does not exist.

- [ ] **Step 3: Add one Core helper and apply it before every Periphery request**

Add in `bin/core/src/api/read/mod.rs`:

```rust
pub(super) const MAX_LOG_TAIL: u64 = 5_000;

pub(super) fn cap_log_tail(tail: u64) -> u64 {
  tail.min(MAX_LOG_TAIL)
}
```

Use `cap_log_tail(tail)` for both Server and Swarm branches in
`GetDeploymentLog`, for both branches in `GetStackLog`, and in
`GetSwarmServiceLog`. Replace the duplicate module-local `MAX_LOG_LENGTH`
constants with the shared helper. For example, the stack Swarm request becomes:

```rust
          periphery_client::api::swarm::GetSwarmServiceLog {
            service: format!("{}_{service}", stack.project_name(false)),
            tail: cap_log_tail(tail),
            timestamps,
            no_task_ids: false,
            no_resolve: false,
            details: false,
          },
```

- [ ] **Step 4: Add defense-in-depth Periphery caps**

Add `const MAX_LOG_TAIL: u64 = 5_000;` beside each
`LOG_COMMAND_TIMEOUT` in compose, container, and swarm service modules. Clamp
the destructured value before formatting a shell command:

```rust
    let tail = tail.min(MAX_LOG_TAIL);
```

All three raw log handlers must contain that line before their `format!` call.

- [ ] **Step 5: Verify all affected crates**

Run: `rtk cargo test -p komodo_core api::read::log_limit_tests`

Expected: 1 test passes.

Run: `rtk cargo test -p komodo_periphery`

Expected: all Periphery tests pass.

Run: `rtk cargo check -p komodo_core -p komodo_periphery`

Expected: Stack, Deployment, container, compose, and swarm log routes compile with a 5,000-line maximum.

- [ ] **Step 6: Commit line caps**

```bash
rtk git add bin/core/src/api/read/mod.rs bin/core/src/api/read/stack.rs bin/core/src/api/read/swarm.rs bin/core/src/api/read/deployment.rs bin/periphery/src/api/compose.rs bin/periphery/src/api/container/mod.rs bin/periphery/src/api/swarm/service.rs
rtk git commit -m "fix: cap all log tail requests"
```

### Task 7: Prove the 100 MiB output RSS bound and close checkpoint 2

**Files:**
- Modify: `lib/command/tests/bounded_output.rs`

- [ ] **Step 1: Add an ignored Linux RSS characterization test**

Append to `lib/command/tests/bounded_output.rs`:

```rust
#[cfg(target_os = "linux")]
fn rss_bytes() -> u64 {
  std::fs::read_to_string("/proc/self/status")
    .unwrap()
    .lines()
    .find_map(|line| line.strip_prefix("VmRSS:"))
    .and_then(|value| value.split_whitespace().next())
    .and_then(|value| value.parse::<u64>().ok())
    .unwrap()
    * 1024
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "staging RSS characterization"]
async fn one_hundred_mib_producer_adds_at_most_thirty_two_mib_rss() {
  use std::sync::{
    Arc, Barrier,
    atomic::{AtomicBool, Ordering},
  };

  let baseline = rss_bytes();
  let stop = Arc::new(AtomicBool::new(false));
  let barrier = Arc::new(Barrier::new(2));
  let sampler = std::thread::spawn({
    let stop = stop.clone();
    let barrier = barrier.clone();
    move || {
      let mut peak = rss_bytes();
      barrier.wait();
      while !stop.load(Ordering::Acquire) {
        peak = peak.max(rss_bytes());
        std::thread::sleep(Duration::from_millis(5));
      }
      peak.max(rss_bytes())
    }
  });
  barrier.wait();
  let output = run_shell_command(
    "yes x | head -c 104857600",
    None,
  )
  .await;
  stop.store(true, Ordering::Release);
  let peak = sampler.join().unwrap();
  let delta = peak.saturating_sub(baseline);
  let retained_output_bytes =
    (output.stdout.len() + output.stderr.len()) as u64;
  let marker_present = output.stdout.contains(OUTPUT_TRUNCATION_MARKER)
    || output.stderr.contains(OUTPUT_TRUNCATION_MARKER);

  if let Ok(path) = std::env::var("COMMAND_RSS_ARTIFACT") {
    let path = std::path::PathBuf::from(path);
    let temporary = path.with_extension("json.tmp");
    std::fs::write(
      &temporary,
      format!(
        "{{\"rss_baseline_bytes\":{baseline},\"rss_peak_bytes\":{peak},\"rss_peak_delta_bytes\":{delta},\"retained_output_bytes\":{retained_output_bytes},\"truncation_marker_present\":{marker_present}}}\n"
      ),
    )
    .unwrap();
    std::fs::rename(temporary, path).unwrap();
  }

  assert!(output.success());
  assert!(marker_present);
  assert!(retained_output_bytes <= 8 * 1024 * 1024 + 4096);
  assert!(
    delta <= 32 * 1024 * 1024,
    "peak RSS delta was {} bytes",
    delta,
  );
}
```

- [ ] **Step 2: Run the release-mode RSS test in Linux**

Run: `rtk cargo test -p command --release --test bounded_output one_hundred_mib_producer_adds_at_most_thirty_two_mib_rss -- --ignored --nocapture`

Expected on the designated Linux staging host: 1 test passes, the concurrent
5 ms sampler observes at most 32 MiB peak RSS growth, and retained output
remains at most 8 MiB. If it fails, capture `/usr/bin/time -v` and reduce
temporary pipe buffers before proceeding; do not weaken the 32 MiB target.

- [ ] **Step 3: Run checkpoint-wide verification**

Run: `rtk cargo fmt --all -- --check`

Expected: exit 0.

Run: `rtk cargo test -p command`

Expected: all non-ignored command tests pass.

Run: `rtk cargo test -p komodo_client --features blocking blocking_poll_tests`

Expected: 2 tests pass.

Run: `rtk cargo test -p komodo_core api::read::log_limit_tests`

Expected: 1 test passes.

- [ ] **Step 4: Commit the RSS test and open checkpoint 2**

```bash
rtk git add lib/command/tests/bounded_output.rs
rtk git commit -m "test: enforce command output rss budget"
rtk git push -u origin runtime-bounded-output
rtk gh pr create --repo intezya/komodo --base main --head runtime-bounded-output --title "Bound command output and client polling" --body "Adds 500 ms blocking-client cadence, an additive deadline API, bounded continuously-drained command pipes, explicit 8 MiB truncation, deterministic timeout cleanup, and 5,000-line log caps. Verification: cargo test -p command; cargo test -p komodo_client --features blocking blocking_poll_tests; cargo test -p komodo_core api::read::log_limit_tests."
```

Expected: fork-only PR created and its Linux RSS characterization evidence is attached to the PR description or comment before merge.

### Task 8: Implement cancellation-safe Core workload budgets and metrics

**Files:**
- Create: `bin/core/src/runtime.rs`
- Modify: `bin/core/src/lib.rs`

- [ ] **Step 1: Write deterministic RED tests for every budget class**

Register `mod runtime;` in `bin/core/src/lib.rs`, and create the tests before
the implementation. Use an injectable `RuntimeLimits`, an injected clock for
the observer, and a `wait_for_metrics` helper; do not use
`yield_now()` as a queue barrier.

The focused suite must contain these named cases:

- `saturated_key_does_not_hold_global_capacity`: hold `server-a`, wait until
  `queued_by_key["server-a"] == 1`, and prove `server-b` acquires the other
  global slot.
- `older_key_waiter_wins`: enqueue two waiters for the same key, release the
  holder, and assert the older waiter is admitted first.
- `leaf_acquires_key_before_global`: instrument the test semaphores and assert
  the only acquisition trace is `key -> global`.
- `cancelled_leaf_waiter_releases_both_queue_slots`: abort a queued waiter and
  wait until both queue gauges return to zero.
- `action_root_does_not_consume_a_leaf_permit` and
  `procedure_root_does_not_consume_a_leaf_or_action_permit`.
- `orchestrator_and_procedure_queue_overflow_are_independent`: fill each class queue
  separately and assert an HTTP 503 with the matching admission label.
- `monitor_admission_never_waits`: the second same-key monitor attempt returns
  `None` and increments `skipped_monitor`.
- `all_raii_gauges_return_to_zero_after_abort_and_panic`.
- `observer_reports_window_bounds_lag_busy_and_per_key_peaks`: pause Tokio
  time, advance one second, and assert one complete metric window.
- `completed_orchestrator_and_procedure_emit_id_keyed_terminal_metrics`: assert
  terminal snapshots contain the exact Update IDs and completed Procedure IDs
  are removed from live maps immediately.

Run: `rtk cargo test -p komodo_core runtime::tests -- --nocapture`

Expected: RED because `RuntimeBudget`, its four admission methods, metrics,
and the observer do not exist.

- [ ] **Step 2: Freeze the four independent admission classes**

Implement these production constants in `runtime.rs`:

```rust
pub const LEAF_GLOBAL_PERMITS: usize = 32;
pub const LEAF_PER_KEY_PERMITS: usize = 4;
pub const SHARED_EXECUTION_QUEUE: usize = 256;
pub const LEAF_PER_KEY_QUEUE: usize = 16;
pub const ORCHESTRATOR_ROOT_PERMITS: usize = 6;
pub const ORCHESTRATOR_ROOT_QUEUE: usize = 32;
pub const PROCEDURE_ROOT_PERMITS: usize = 8;
pub const PROCEDURE_ROOT_QUEUE: usize = 32;
pub const MONITOR_GLOBAL_PERMITS: usize = 16;
pub const MONITOR_PER_KEY_PERMITS: usize = 2;
pub const EXECUTION_ADMISSION_TIMEOUT: Duration =
  Duration::from_secs(5);
```

`RuntimeBudget` owns separate fair semaphores for leaf-global, leaf-per-key,
orchestrator roots, Procedure roots, monitor-global, and monitor-per-key work.
An orchestrator root is any execution that waits for child executions:
`RunAction`, `RunBuild`, `RunSync`, or `DeployStackIfChanged`. It also owns one
shared 256-slot execution queue, the Orchestrator and Procedure
32-slot class queues, and lazily-created 16-slot leaf queues per work key.
Store per-key semaphore bundles behind `Weak` entries and prune dead entries
on insertion/observer ticks; deleted resource IDs must not create a
process-lifetime cardinality leak.

Expose exactly these methods:

```rust
pub async fn acquire_leaf(
  &self,
  work_key: Option<&str>,
) -> mogh_error::Result<LeafPermit>;

pub async fn acquire_orchestrator_root(
  &self,
) -> mogh_error::Result<OrchestratorRootPermit>;

pub async fn acquire_procedure_root(
  &self,
) -> mogh_error::Result<ProcedureRootPermit>;

pub async fn try_monitor(
  &self,
  work_key: &str,
) -> Option<MonitorPermit>;
```

`acquire_leaf` performs this order under one absolute five-second deadline:

1. reserve the per-key queue slot, when a key exists;
2. reserve the shared execution queue slot;
3. await the fair per-key semaphore;
4. await the fair global leaf semaphore.

Never probe a semaphore with `try_acquire` before joining its fair wait queue.
Orchestrator and Procedure roots reserve their class queue and the shared queue, then
await only their own class semaphore. They never consume a leaf permit. Monitor
admission uses non-waiting `try_acquire_owned`; if either permit is
unavailable, release the other one and record a skip. Every permit and queue
reservation is an owned RAII guard so cancellation, panic, timeout, and normal
completion use the same release path.

`RunBuild` is deliberately two-phase. Its lifecycle holds the orchestrator
permit, then acquires a normal `acquire_leaf(builder_work_key)` permit only
around the actual Builder/server build command and releases that keyed/global
leaf permit before starting post-build redeploy children. Thus up to six build
orchestrators may exist, but actual work still satisfies global 32 and
same-builder 4; redeploy children cannot wait behind a leaf held by their
parent. No other orchestrator acquires a leaf for its own host process.

Keep `execution_work_key(&Update, &AllResourcesById) -> Option<String>` in
this module. It maps Server directly, Deployment/Stack through configured
server or first Swarm server, Build through Builder, Repo according to its
operation, and uses stable synthetic `swarm:{id}` / `builder:{id}` keys
when there is no concrete server. Read resources only through Plan 1's
dirty-aware `all_resources_cache().read().await`.

Add the imports required by the complete file rather than relying on sibling
modules: `BTreeMap`, `HashMap`, `Arc`, `LazyLock`,
`std::sync::Mutex`, `AtomicU64`, `AtomicUsize`, `Duration`,
`serde::Serialize`, `tokio::runtime::Handle`,
`OwnedSemaphorePermit`, and `Semaphore`, plus the concrete Komodo entity
types used by `execution_work_key`. Keep async cache locks out of RAII
`Drop` implementations.

- [ ] **Step 3: Make every class and key observable**

Back `RuntimeMetrics` with atomics plus a short synchronous
`std::sync::Mutex<HashMap<String, KeyMetrics>>` for per-key counters. The
mutex protects only integer updates/snapshots and is never held over an await;
this permits exact decrementing from `Drop`. After a window snapshot, remove
keys whose queued/active gauges and reset peaks are all zero.

Expose a serializable snapshot containing at least:

```rust
#[derive(Clone, Debug, serde::Serialize)]
pub struct RuntimeMetricsSnapshot {
  pub queued_execution_total: usize,
  pub peak_queued_execution_total: usize,
  pub queued_leaf: usize,
  pub peak_queued_leaf: usize,
  pub active_leaf: usize,
  pub peak_leaf: usize,
  pub queued_by_key: BTreeMap<String, usize>,
  pub peak_queued_by_key: BTreeMap<String, usize>,
  pub active_by_key: BTreeMap<String, usize>,
  pub peak_by_key: BTreeMap<String, usize>,
  pub queued_orchestrator: usize,
  pub peak_queued_orchestrator: usize,
  pub active_orchestrator: usize,
  pub peak_orchestrator: usize,
  pub queued_procedure_root: usize,
  pub peak_queued_procedure_root: usize,
  pub active_procedure_root: usize,
  pub peak_procedure_root: usize,
  /// Aggregate across active roots; per-root limits are checked below.
  pub procedure_ready: usize,
  pub procedure_nodes: usize,
  pub active_procedure_expand: usize,
  pub peak_procedure_expand: usize,
  pub active_procedure_tree_leaf: usize,
  pub peak_procedure_tree_leaf: usize,
  pub procedure_by_root:
    BTreeMap<String, ProcedureRootMetricsSnapshot>,
  pub active_monitor: usize,
  pub peak_monitor: usize,
  pub active_monitor_by_key: BTreeMap<String, usize>,
  pub peak_monitor_by_key: BTreeMap<String, usize>,
  pub rejected_leaf: u64,
  pub rejected_orchestrator: u64,
  pub rejected_procedure_root: u64,
  pub skipped_monitor: u64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ProcedureRootMetricsSnapshot {
  pub root_update_id: String,
  pub ready: usize,
  pub peak_ready: usize,
  pub nodes: usize,
  pub active_expand: usize,
  pub peak_expand: usize,
  pub active_leaf: usize,
  pub peak_leaf: usize,
}
```

Provide scheduler-only RAII methods to change
`procedure_ready`, `procedure_nodes`, expansion, and tree-leaf gauges; the
scheduler in Task 9 supplies the root Update ID and must not reach into atomics
directly. On root completion call
`finish_procedure_metrics(root_update_id)`, which atomically removes and
returns that root snapshot, then emit a separate structured
`procedure_root_metrics` terminal event with Update ID, outcome, duration,
and all root peaks. Do not retain completed IDs until the next observer tick.
The one-second `procedure_by_root` map therefore contains only active roots,
while terminal per-Procedure evidence is lossless and cardinality remains
bounded by eight. Other peaks and window counters reset only after an observer
snapshot; live gauges do not reset. Reset each peak to its simultaneously
observed live gauge, not zero, so work spanning a window remains represented.
Use a compare-exchange reset loop (and the per-key mutex) so a concurrent
`fetch_max` cannot be overwritten by observer reset; add a reset/admission
race test.

Have orchestrator admission return its queue-wait timestamp/duration. When the
dispatcher finishes an orchestrator, emit one structured
`orchestrator_root_metrics` event keyed by Update ID, operation, and target with
admission wait, run duration, outcome, and the class active/peak snapshot. This
supplies per-root evidence without retaining unbounded IDs in the one-second
gauge map.

- [ ] **Step 4: Emit one-second Core runtime windows**

Implement `spawn_runtime_observer()` and call it exactly once after the Core
Tokio runtime has started. Use absolute 100 ms ticks so drift does not hide
scheduler stalls. Every one-second window logs a structured event whose exact
event field is `runtime_budget_metrics` and includes:

- `window_start_unix_ms` and `window_end_unix_ms`;
- nearest-rank `tokio_lag_p99_ms` from the samples in that same window;
- `tokio_busy_pct`, calculated from the delta of
  `Handle::current().metrics().worker_total_busy_duration(worker)`, divided
  by elapsed wall time times `num_workers()`;
- the complete `RuntimeMetricsSnapshot`.

Clamp only rounding noise above 100%; do not convert a missing sample to zero.
The staging gate must reject windows without lag or busy data. Add an internal
`RuntimeWindowObserver` seam so tests can inject busy-duration snapshots and
timestamps without starting a second process-wide observer.

- [ ] **Step 5: Verify and commit the primitive**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo test -p komodo_core runtime::tests -- --nocapture
rtk cargo check -p komodo_core
```

Expected: all runtime tests pass; observed active/peak values never exceed
32 leaf, 4 per key, 6 orchestrator, 8 Procedure root, 16 monitor, or 2 monitor per
key; shared/class/per-key queue peaks stay within 256/32/16; all cancellation
gauges return to zero.

Commit:

```bash
rtk git add bin/core/src/runtime.rs bin/core/src/lib.rs
rtk git commit -m "feat: add observable runtime work budgets"
```

### Task 9: Route every execution origin through one dispatcher and bounded tree scheduler

**Files:**
- Create: `bin/core/src/api/execute/dispatch.rs`
- Create: `bin/core/src/helpers/procedure_tree.rs`
- Modify: `bin/core/src/api/execute/mod.rs`
- Modify: `bin/core/src/api/execute/action.rs`
- Modify: `bin/core/src/api/execute/build.rs`
- Modify: `bin/core/src/api/execute/procedure.rs`
- Modify: `bin/core/src/api/execute/stack.rs`
- Modify: `bin/core/src/api/execute/sync.rs`
- Modify: `bin/core/src/api/write/sync.rs`
- Modify: `bin/core/src/helpers/procedure.rs`
- Modify: `bin/core/src/helpers/update.rs`
- Modify: `bin/core/src/helpers/mod.rs`
- Modify: `bin/core/src/schedule.rs`
- Modify: `bin/core/src/startup.rs`
- Modify: `bin/core/src/api/listener/resources.rs`
- Modify: `bin/core/src/sync/deploy.rs`
- Modify: `bin/core/src/monitor/mod.rs`
- Modify: `bin/core/src/monitor/swarm.rs`
- Modify: `docsite/docs/automate/procedures.md`

- [ ] **Step 1: Define the single root-execution boundary**

Create `dispatch.rs` with these public inputs and one result type:

```rust
pub enum ExecutionOrigin {
  Api,
  Schedule,
  Startup,
  Webhook,
  SyncDeploy,
}

pub enum DispatchMode {
  Detached,
  Await,
}

pub async fn dispatch_execution(
  request: ExecuteRequest,
  user: User,
  origin: ExecutionOrigin,
  mode: DispatchMode,
) -> mogh_error::Result<ExecutionResult>
{ /* preflight, initialization, admission, execution */ }

pub(crate) struct ChildLifecycle<T> {
  cancel: CancellationToken,
  completed: Option<oneshot::Receiver<anyhow::Result<T>>>,
}

pub(crate) enum ChildWait<T> {
  Completed(T),
  Cancelled,
}

impl<T> ChildLifecycle<T> {
  pub async fn wait_with_parent_cancel(
    mut self,
    parent_cancel: &CancellationToken,
  ) -> anyhow::Result<ChildWait<T>>;
}

pub(crate) async fn dispatch_child_execution(
  request: ExecuteRequest,
  user: User,
  origin: ExecutionOrigin,
  parent_cancel: &CancellationToken,
) -> mogh_error::Result<ChildLifecycle<ExecutionResult>>;

pub(crate) enum InternalProcedureChild {
  CommitSync(CommitSync),
  Sleep(Duration),
}

pub(crate) enum InternalProcedureOutcome {
  CommitSync(Update),
  Sleep,
}

pub(crate) async fn dispatch_internal_procedure_child(
  child: InternalProcedureChild,
  user: User,
  parent_cancel: &CancellationToken,
) -> anyhow::Result<ChildLifecycle<InternalProcedureOutcome>>;
```

Use the server's existing `api::execute::ExecuteRequest` enum directly; do
not invent a generic marker-trait conversion. Generic API endpoints convert
their typed request into that enum at the thin `inner_handler` boundary, and
scheduler/listener/sync callers already construct the same enum.

Both entry points transfer the initialized Update to the same owned lifecycle
task. Public/internal root dispatch keeps the existing caller-independent
semantics. `dispatch_child_execution` instead creates a child token from
`parent_cancel`, installs it in the owned task, and returns a lifecycle handle
after ownership transfer. The owned task selects cancellation while queued and
while executing; cancellation drops the admission/resolver future (therefore
killing command process groups), persists and broadcasts exactly one terminal
Cancelled Update, releases permits, and only then resolves `completed`.
`wait_with_parent_cancel` takes the receiver from the `Option`, selects it
against parent cancellation, cancels the child when the parent wins, and then
continues awaiting that same receiver until acknowledgement. Implement `Drop`
for `ChildLifecycle<T>` to cancel its token unconditionally; the owned task
still finishes cleanup if the handle or wait future is dropped.

`CommitSync` and `Sleep` are not server `ExecuteRequest` variants and must not
be converted into fake requests. `dispatch_internal_procedure_child` uses the
same owned-task, keyless leaf admission, child-token, panic conversion, and
completion-ack machinery. `Sleep` returns `InternalProcedureOutcome::Sleep`
without a child Update; its owning Procedure node records/finalizes the result.
`CommitSync` retains its existing distinct audit Update and returns it in
`InternalProcedureOutcome::CommitSync`.

In `api/write/sync.rs`, extract the existing resolver into
`prepare_commit_sync(request, &user)` and
`run_prepared_commit_sync(prepared, &mut update)`. Preparation performs all
validation and pre-Update reads. One shared `run_commit_sync_lifecycle` then
creates/persists the initial CommitSync Update, runs the prepared body, and on
success, returned error, panic, or cancellation appends the correct log,
finalizes, and calls `update_update` exactly once before acknowledging. The
public Write resolver awaits that lifecycle with a standalone token; the
Procedure internal dispatcher uses a child of the root token. Neither path may
retain the old early-return `add_update` terminal writes. Add tests cancelling
after initial persistence and during file/git work: the exact CommitSync ID has
one initial and one terminal event, while Sleep creates no child Update and
only the owning Procedure node finalizes.

The dispatcher is the only root path allowed to:

1. reject a prohibited `ExecutionOrigin::Api` request from `action_user()`
   before request preflight, Update initialization, or queue reservation;
2. validate cancellation and permissions, then run request preflight;
3. classify `RunAction`, `RunBuild`, `RunSync`, and
   `DeployStackIfChanged` as orchestrator roots, `RunProcedure` as a Procedure
   root, and all non-batch operations that do not await children as leaf work;
   mark `RunBuild` for its additional keyed leaf build phase;
4. initialize the Update only after those preflight decisions;
5. resolve a work key and acquire the matching runtime permit;
6. spawn a detached task or await the same task body;
7. retain the owned permit until the operation and terminal persistence finish;
8. turn queue-full/timeout into HTTP 503 and a terminal Update.

Immediately after initialization returns an Update, with no intervening await,
move that Update into one owned Tokio lifecycle task. The task—not the HTTP or
internal caller—performs admission, execution, panic conversion, terminal
persistence, and guard release. It owns an admission-result oneshot and a
completion oneshot. On queue-full/timeout it first persists the terminal 503
Update and then sends the admission error; if the caller disappeared, the send
failure does not stop lifecycle cleanup. On success it sends the admitted
initial response while retaining the permit. `Detached` awaits only that
admission oneshot; `Await` then awaits completion. Dropping either caller while
admission is queued therefore leaves an owned task that will admit/execute or
persist rejection—never an orphaned InProgress Update. Panic is converted into
a terminal Update before the task releases its guard.

Add cancellation tests at three points for every class: immediately after
Update initialization, while waiting in a full queue, and after admission.
Each exact Update ID eventually has one terminal persistence/event and every
queue/active gauge returns to zero. The implementation test seam pauses the
lifecycle task after ownership transfer; it must not insert a second test-only
Update path.

Do **not** add an admission field to the shared `ExecuteArgs`; its existing
98 struct literals remain source-compatible. The dispatcher owns root guards
in its task frame. Ordinary leaf resolvers continue receiving the unchanged
`ExecuteArgs`. Procedure-node state is private to the scheduler API below.

Keep batch parent requests outside all root permits. Their children call
`dispatch_execution` by enumerating inputs, running
`stream::iter(...).buffer_unordered(32)`, collecting `(index, result)`,
sorting by index, and stripping the index. This keeps 32 slots utilized while
restoring input order. The bound covers permission checks,
Update insertion, queue waiting, and spawn/await, not just resolver futures.
Add table-driven test
`dispatch_batch_matrix_10_100_1000`; response order for 10, 100, and 1,000
inputs must match input order and peak preflight must be 10, 32, and 32
respectively.

- [ ] **Step 2: Persist every admission rejection, including empty Update IDs**

Move this helper to `helpers/update.rs` and inject persistence functions in
unit tests:

```rust
pub async fn persist_admission_rejection(
  mut update: Update,
  label: &'static str,
  error: &anyhow::Error,
) -> anyhow::Result<()> {
  update.push_error_log(label, error.to_string());
  update.finalize();
  if update.id.is_empty() {
    add_update(update).await.map(|_| ())
  } else {
    update_update(update).await
  }
}
```

Use labels `Execution admission`, `Orchestrator admission`, and
`Procedure admission`. An empty-ID route such as
`DeployStackIfChanged` inserts and broadcasts its rejection; an initialized
route replaces its Update. Log a persistence error separately and return the
original 503. Test both branches and assert exactly one terminal event.

- [ ] **Step 3: Migrate API, scheduler, startup, webhooks, and sync deploy**

Make `api/execute/mod.rs::inner_handler` a thin API adapter over
`dispatch_execution(..., Api, Detached)`. Replace every
`init_execution_update(...)` plus direct `.resolve(&ExecuteArgs { ... })`
root sequence in these files:

- `schedule.rs`: dispatch with `Schedule, Await`; use a bounded
  `JoinSet`/stream of at most 32 scheduled submissions.
- `startup.rs`: dispatch startup executions with `Startup, Await`; replace
  the execution `join_all` at the current startup fan-out, without changing
  unrelated Write request resolution.
- `api/listener/resources.rs`: dispatch all webhook/listener execution
  requests with `Webhook, Await`; leave Read/Write request resolution alone.
- `sync/deploy.rs`: dispatch both Stack and Deployment sync executions with
  `SyncDeploy, Await`; replace `join_all(good_to_deploy...)` with
  enumerate → `buffer_unordered(32)` → collect → sort-by-index, so
  input/result order is retained without head-of-line underutilization.
- `api/execute/build.rs`: replace post-build auto-redeploy's direct Update
  initialization/resolution and unbounded `join_all` with indexed
  `dispatch_child_execution(..., SyncDeploy, lifecycle_cancel)` submissions through
  `buffer_unordered(32)`, retaining deployment result order. Split the resolver
  at the existing post-build boundary: while holding the root orchestrator
  permit, `run_build_phase` acquires the computed Builder/server leaf key,
  performs the real build, and releases that `LeafPermit`; only then may
  `run_post_build_redeploy` submit child Deployment lifecycle tasks. Neither
  helper finalizes the root Update independently.
- `api/execute/stack.rs`: route FullDeploy and the service deploy/restart
  helpers reached by `DeployStackIfChanged` through the dispatcher's cancellable internal
  initialized-child path. The parent is an orchestrator root; every child gets
  ordinary leaf admission and its normal Update lifecycle. Do not recursively
  dispatch `DeployStackIfChanged` itself and do not reuse a parent leaf permit.
- `api/execute/sync.rs`: route every execution emitted by `RunSync` through the
  same cancellable child-dispatch path and propagate its lifecycle token.
  `RunSync` is an orchestrator root and never
  holds a leaf while awaiting those children.

No origin may manually initialize a root Update, call a root Execute resolver,
or spawn untracked root work after this step. Add
`dispatch::tests::root_execution_source_closure`, which recursively scans every
Rust file below `bin/core/src`. Each occurrence of
`init_execution_update(` or `.resolve(&ExecuteArgs` must match an exact
reviewed `(relative path, owning function, line text)` allowlist containing
only the helper definitions/tests in `helpers/update.rs`, the shared lifecycle
body in `dispatch.rs`, and the private Procedure child executor in
`procedure_tree.rs`; allowing an entire file is forbidden. The test separately
rejects direct `CommitSync.resolve(&WriteArgs` in Procedure helpers and requires
the shared lifecycle in `api/write/sync.rs`. It also
rejects execution fan-out `join_all` in `api/execute`, `schedule.rs`,
`startup.rs`, listeners, and sync deploy. It also proves Schedule, Startup,
Webhook, SyncDeploy, post-build redeploy, and internal Stack/Sync children use
`DispatchMode::Await`; only the API adapter may request Detached. Any new match
fails until its exact ownership is reviewed.

- [ ] **Step 4: Prove Action leaf reentrancy and fail fast on orchestrator cycles**

An Action task owns one `OrchestratorRootPermit` for its full lifetime. Each API
request made by the Action comes back through the dispatcher and acquires a
normal leaf permit; the Action itself never owns a leaf permit. Add a
deterministic regression that:

1. fills all 32 leaf permits;
2. starts six Actions;
3. observes `active_orchestrator == 6` while `active_leaf == 32`;
4. releases leaf work and lets every Action issue and complete at least two
   leaf API calls;
5. asserts no admission timeout and all gauges return to zero.

Also run the inverse case: six active Actions must not delay an unrelated leaf
request when leaf capacity is free.

Actions currently receive temporary credentials for the reserved
`action_user()`. At dispatcher preflight, before Update initialization or any
queue reservation, apply the rejection only when
`origin == ExecutionOrigin::Api && user.id == action_user().id`. Reject
`RunAction`, `BatchRunAction`, `RunBuild`, `BatchRunBuild`, `RunSync`,
`DeployStackIfChanged`, `BatchDeployStackIfChanged`, `RunProcedure`, and every
Procedure batch/root variant. Return stable HTTP 409 error
`Action scripts cannot start orchestration roots`. Scheduled, startup, and
webhook Actions legitimately use `action_user()` with a non-API origin and
must remain allowed; add explicit allow tests for all three. All Read, Write,
and non-orchestrating leaf Execute APIs remain available. Document this
cycle-prevention rule in `docsite/docs/automate/procedures.md`.

Add watchdog tests shorter than 500 ms in which six Actions concurrently call
each prohibited orchestrator variant, and Procedure -> Action -> RunProcedure.
Every nested request must
fail before creating an Update,
`queued_orchestrator`/`queued_procedure_root` stay
zero for the nested requests, parent Actions complete with their normal command
failure logs, and all root/leaf gauges return to zero. This fail-fast policy is
the cycle-free contract; a five-second admission timeout is not accepted as
cycle resolution.

Add two more sub-500 ms saturation watchdogs: six concurrent `RunSync` roots
each dispatch a leaf child, and six concurrent `DeployStackIfChanged` roots
take the FullDeploy child path. Repeat both when invoked as Procedure nodes.
All children make progress after leaf capacity is released; those RunSync and
DeployStackIfChanged parents consume no leaf permit, and a five-second timeout
is never the mechanism that breaks a cycle.

For six same-Builder `RunBuild` roots, assert `active_orchestrator == 6` while
the heavy phase's `active_by_key[builder_key]` and peak never exceed four.
After each build phase releases its leaf, all bounded post-build redeploy tasks
finish under the same watchdog; no redeploy begins while its parent still owns
that keyed leaf.

- [ ] **Step 5: Implement the non-recursive Procedure tree scheduler**

Create `helpers/procedure_tree.rs` with one process-wide
`ProcedureTreeScheduler`:

```rust
pub const PROCEDURE_EXPANSION_WORKERS: usize = 8;
pub const PROCEDURE_LEAF_WORKERS: usize = 32;
pub const PROCEDURE_READY_PER_ROOT: usize = 256;
pub const PROCEDURE_NODES_PER_ROOT: usize = 4_096;

pub async fn submit(
  &self,
  root: ProcedureRootRequest,
) -> anyhow::Result<Update>;
```

The scheduler consists of one coordinator, eight expansion workers, and 32
leaf workers. The coordinator owns all mutable per-root state: node IDs,
parent/dependency counts, stage cursors, completion/error state, cancellation,
a ready `VecDeque` capped at 256, total node count capped at 4,096, and the
root completion oneshot.

Import `HashMap`/`VecDeque`, `std::panic::AssertUnwindSafe`,
`futures_util::{FutureExt, StreamExt}` (the former for task panic capture),
`tokio::sync::{mpsc, oneshot}`, `tokio_util::sync::CancellationToken`,
and the concrete `ExecuteArgs`, `ExecuteRequest`, `Update`, and runtime
permit types in the files that use them. Register `pub mod procedure_tree;`
in `helpers/mod.rs` and `pub(crate) mod dispatch;` in `api/execute/mod.rs` so
the scheduler/startup/listener/sync callers can use it; remove
the old `join_all` imports as their last execution uses disappear.

Workers are pull-driven and receive at most one job at a time. Give each worker
a capacity-one inbox. Expansion and leaf result channels have capacity equal
to their worker counts, so one return slot is reserved before a job is
assigned. The coordinator uses `try_send` only to idle workers and always
selects result/cancellation input before dispatching more ready work; it never
awaits a send into a full worker queue.

An expansion worker resolves exactly one Procedure node/stage into child
descriptors, sends that bounded `ExpansionResult`, and returns idle. It does
not await, join, or retain a permit for any child. The coordinator validates
the node/ready limits and classifies jobs:

- nested `RunProcedure` is another expansion node and acquires no root permit;
- every server Execute descriptor calls `dispatch_child_execution` with the
  root token and awaits its `ChildLifecycle`; the dispatcher classifies
  `RunAction`/`RunBuild`/`RunSync`/`DeployStackIfChanged` as orchestrators and
  ordinary requests as keyed leaves in normative per-key-then-global order;
  `RunBuild` alone acquires the additional keyed leaf for its heavy subphase;
- `CommitSync` and `Sleep` call `dispatch_internal_procedure_child`, await the
  same cancellation/completion contract, and use keyless leaf admission
  without inventing a server Execute request; CommitSync returns its real
  audit Update, while Sleep returns unit-like outcome and creates none.

Each server Execute child uses the same dispatcher preflight, Update
initialization, terminal-rejection, resolver, and terminal-persistence helpers
as a root request. Internal CommitSync/Sleep use their explicitly scoped
lifecycle above. The tree worker supplies only user and parent token and
awaits the resulting typed handle. This avoids a second implementation of
dispatch and prevents a resolver bypass. After completion the worker reports
`ChildDone` and returns idle.

An orchestrator child may occupy a tree worker while waiting for one of the six
orchestrator permits, but its own leaf/redeploy/sync children never re-enter the
32 tree-worker queue. They run in independently supervised dispatcher lifecycle
tasks, so even 32 waiting mixed orchestrator nodes leave child execution
capacity. The dispatcher passes the owning lifecycle token into RunBuild,
RunSync, and DeployStackIfChanged fan-out helpers; every descendant uses a
child token and completes its terminal acknowledgement before its parent
lifecycle resolves. The saturation watchdog asserts external child progress
and that no child job is enqueued onto the tree coordinator.

Pass the root `CancellationToken` into every assigned expansion and execution
job. A local expansion job uses a biased `tokio::select!` and may be dropped on
cancellation. For a dispatcher-owned Execute or internal child, the worker
obtains its typed `ChildLifecycle` and calls
`wait_with_parent_cancel(&root_cancel)`. Return
`ChildCancelled { root_id, node_id, kind }` only
after the owned lifecycle releases its permits and acknowledges. Server
Execute and CommitSync lifecycles terminal-persist/broadcast their child Update
exactly once before that acknowledgement; Sleep acknowledges outcome/progress
without a child Update, after which the parent node finalizer persists its
terminal state. Never drop only the completion receiver. The dispatcher
task drops command/Sleep resolver futures immediately on its token, so a
multi-hour Sleep is cancelled while descendant terminal acknowledgement is
still guaranteed. A panic becomes a
`WorkerPanicked { root_id, node_id, kind }` result through the worker's
already-reserved result slot; the worker then returns idle. The coordinator
cancels that root and executes normal terminal/drain handling. No worker panic
may silently consume a slot or strand the root oneshot.

A `RunProcedure` child never recursively calls the top-level dispatcher and
never acquires a second root permit. Expose a private
`resolve_procedure_node(request, user, update, &mut ProcedureNodeState)`
between `api/execute/procedure.rs`, `helpers/procedure.rs`, and the
scheduler. This private state is the only carrier for tree bookkeeping; there
is no `procedure_admission` or other new field on `ExecuteArgs`.

The dispatcher's owned task acquires one `ProcedureRootPermit`, keeps it in
that task frame, and awaits `submit(root)` until the coordinator emits the
root terminal result. The permit is deliberately not passed through
`ExecuteArgs` or stored in recursive state. Node-limit
or ready-limit overflow cancels remaining jobs, appends one
`Procedure scheduler overload` error, cancels the root token, drains the now
bounded `ChildCancelled`/completion results from assigned workers,
and persists one terminal Update. Parent cancellation does the same without
leaking permits.

Store the canonical Procedure resource ID in each node's ancestry path.
Reject `A -> B -> A` (and direct self-reference) as a cycle before enqueueing
the repeated node; the 4,096-node cap is an overload boundary, not the normal
cycle detector. Nested nodes retain their existing child Update lifecycle and
terminal events, while the root completion oneshot resolves only after every
required descendant terminal persistence finishes.

- [ ] **Step 6: Test branching, nesting, cancellation, and overload**

Add scheduler tests using injected leaf/expansion functions:

- a 40-level single-child chain completes without recursion;
- a four-level tree with branching factor 8 (585 total nodes) completes and never exceeds eight
  expansion or 32 leaf workers;
- eight simultaneous roots whose first child is another Procedure all reach a
  leaf; no root or worker waits on a descendant;
- a parent with multiple nested stages advances only after all dependencies in
  the prior stage report `ChildDone`;
- cancellation during expansion and cancellation during leaf/orchestrator
  execution returns `ChildCancelled`, drains worker results, and returns all
  gauges to zero; a multi-hour `Sleep` child cancels and reaches leaf-to-root
  terminal finalization under a 500 ms watchdog; repeat with in-flight
  post-build redeploy and RunSync child lifecycles and assert every descendant
  terminal event precedes the root terminal event;
- injected expansion and leaf panics produce one terminal error, keep worker
  pools at full configured size, and never strand the root oneshot;
- 4,097 nodes and 257 ready jobs each produce one terminal overload Update;
- `CommitSync` and `Sleep` pass through keyless leaf admission; CommitSync has
  exactly one initial/terminal child audit Update and Sleep has none;
- a Procedure stage containing 32 mixed `RunAction`, `RunBuild`, `RunSync`, and
  `DeployStackIfChanged` children never reports them as leaf work, peaks at six
  orchestrator permits, keeps the real build subphase at global 32 and
  same-builder 4, and lets independently dispatched leaf children/API calls
  finish even while all tree workers await orchestrators;
- six orchestrator roots calling leaf APIs concurrently with eight branching
  Procedure roots complete without circular wait;
- `procedure_batch_matrix_10_100_1000` runs 10, 100, and 1,000 entries and
  keeps global root/worker metrics and
  every `procedure_by_root` ready/node/worker metric inside its fixed bound.

The branching and reentrant cases must use a watchdog shorter than the
five-second admission deadline so a permit cycle fails the test immediately.

- [ ] **Step 7: Keep monitor fan-out non-blocking**

In `monitor/mod.rs` and `monitor/swarm.rs`, preserve Plan 1's single
collection-sized inventory preload. Wrap each refresh in
`runtime_budget().try_monitor(work_key)`; on `None`, emit structured
`monitor_work_skipped` and retry only on the next normal interval. Execute
accepted refreshes with `buffer_unordered(MONITOR_GLOBAL_PERMITS)`. Do not
call targeted public refresh wrappers from the all-resource loops, because
that restores the eliminated relationship queries.

- [ ] **Step 8: Run integration and source-closure gates**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo test -p komodo_core runtime::tests
rtk cargo test -p komodo_core procedure_tree::tests -- --nocapture
rtk cargo test -p komodo_core execute::dispatch::tests -- --nocapture
rtk cargo test -p komodo_core execute::dispatch::tests::root_execution_source_closure -- --exact
rtk cargo test -p komodo_core
rtk cargo check -p komodo_core
rtk rg -n 'init_execution_update|\.resolve\(&ExecuteArgs|CommitSync.*resolve\(&WriteArgs' bin/core/src
```

Expected: tests and checks pass. The final `rg` prints only the exact reviewed
helper-definition/test and dispatcher/Procedure-child allowlist that the source
closure test also validates; it has no unowned root Execute call. A second
source assertion checks every internal origin and child path uses
`DispatchMode::Await`.

Commit:

```bash
rtk git add \
  bin/core/src/runtime.rs \
  bin/core/src/api/execute/dispatch.rs \
  bin/core/src/api/execute/mod.rs \
  bin/core/src/api/execute/action.rs \
  bin/core/src/api/execute/build.rs \
  bin/core/src/api/execute/procedure.rs \
  bin/core/src/api/execute/stack.rs \
  bin/core/src/api/execute/sync.rs \
  bin/core/src/api/write/sync.rs \
  bin/core/src/helpers/procedure_tree.rs \
  bin/core/src/helpers/procedure.rs \
  bin/core/src/helpers/update.rs \
  bin/core/src/helpers/mod.rs \
  bin/core/src/schedule.rs \
  bin/core/src/startup.rs \
  bin/core/src/api/listener/resources.rs \
  bin/core/src/sync/deploy.rs \
  bin/core/src/monitor/mod.rs \
  bin/core/src/monitor/swarm.rs \
  docsite/docs/automate/procedures.md
rtk git commit -m "perf: route execution through bounded runtime"
```

### Task 10: Reserve Core and Periphery child-process capacity by workload class

**Files:**
- Modify: `Cargo.lock`
- Modify: `lib/command/Cargo.toml`
- Create: `lib/command/src/budget.rs`
- Modify: `lib/command/src/lib.rs`
- Modify: `lib/command/tests/bounded_output.rs`
- Modify: `bin/core/src/api/execute/action.rs`
- Modify: `bin/periphery/src/docker/mod.rs:294-325`
- Modify: `bin/periphery/src/docker/compose.rs:1-40`
- Modify: `bin/periphery/src/api/container/mod.rs:420-620`
- Create: `bin/periphery/src/runtime_metrics.rs`
- Modify: `bin/periphery/src/stats.rs`
- Modify: `bin/periphery/src/lib.rs`

- [ ] **Step 1: Write failing command-budget tests**

Create `lib/command/src/budget.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
  use std::{sync::Arc, time::Duration};

  use super::*;

  #[tokio::test]
  async fn monitor_capacity_is_reserved_from_user_work() {
    let budget = Arc::new(CommandBudget::new(CommandLimits {
      user_active: 1,
      user_queue: 1,
      user_wait: Duration::from_millis(50),
      action_host_active: 1,
      monitor_active: 1,
    }));
    let _user = budget.acquire(CommandClass::User).await.unwrap();
    assert!(budget.acquire(CommandClass::Monitor).await.is_ok());
  }

  #[tokio::test]
  async fn monitor_work_skips_when_its_reserved_slot_is_full() {
    let budget = CommandBudget::new(CommandLimits {
      user_active: 1,
      user_queue: 1,
      user_wait: Duration::from_millis(50),
      action_host_active: 1,
      monitor_active: 1,
    });
    let _monitor = budget.acquire(CommandClass::Monitor).await.unwrap();
    assert!(matches!(
      budget.acquire(CommandClass::Monitor).await,
      Err(CommandAdmissionError::MonitorBusy)
    ));
  }

  #[tokio::test]
  async fn abort_releases_command_permit_and_metrics() {
    // Queue behind a held user permit, wait for queued_user == 1, abort the
    // waiter, and assert the queued gauge returns to zero.
  }

  #[tokio::test]
  async fn action_host_capacity_is_independent_from_user_work() {
    // Fill ActionHost, prove User still admits, then fill User and prove an
    // already-admitted ActionHost remains independent.
  }

  #[tokio::test]
  async fn command_batch_matrix_10_100_1000() {
    // Run injected batches of 10, 100, and 1_000 commands. Preserve result
    // order and assert peak_user == min(batch_size, 6).
  }
}
```

- [ ] **Step 2: Run tests and verify RED**

Register `mod budget;` in `lib/command/src/lib.rs`, then run:

Run: `rtk cargo test -p command budget::tests`

Expected: compilation fails because budget types are not defined.

- [ ] **Step 3: Implement separate Action-host, user, and monitor process budgets**

Add above the tests in `lib/command/src/budget.rs`:

```rust
use std::{
  sync::{Arc, LazyLock},
  time::Duration,
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const USER_COMMAND_PERMITS: usize = 6;
pub const USER_COMMAND_QUEUE: usize = 64;
pub const USER_COMMAND_WAIT: Duration = Duration::from_secs(5);
pub const ACTION_HOST_COMMAND_PERMITS: usize = 6;
pub const MONITOR_COMMAND_PERMITS: usize = 2;

#[derive(Debug, Clone, Copy)]
pub(crate) enum CommandClass {
  User,
  ActionHost,
  Monitor,
}

#[derive(Debug, thiserror::Error)]
pub enum CommandAdmissionError {
  #[error("periphery user command queue is full")]
  UserQueueFull,
  #[error("periphery user command admission timed out")]
  UserTimeout,
  #[error("periphery monitor command capacity is busy")]
  MonitorBusy,
  #[error("Core Action host command capacity is busy")]
  ActionHostBusy,
  #[error("periphery command budget closed")]
  Closed,
}

#[derive(Clone, Copy)]
struct CommandLimits {
  user_active: usize,
  user_queue: usize,
  user_wait: Duration,
  action_host_active: usize,
  monitor_active: usize,
}

pub struct CommandBudget {
  limits: CommandLimits,
  user_active: Arc<Semaphore>,
  user_queue: Arc<Semaphore>,
  action_host_active: Arc<Semaphore>,
  monitor_active: Arc<Semaphore>,
  metrics: Arc<CommandMetrics>,
}

impl CommandBudget {
  fn new(limits: CommandLimits) -> Self {
    Self {
      limits,
      user_active: Arc::new(Semaphore::new(limits.user_active)),
      user_queue: Arc::new(Semaphore::new(limits.user_queue)),
      action_host_active: Arc::new(Semaphore::new(limits.action_host_active)),
      monitor_active: Arc::new(Semaphore::new(limits.monitor_active)),
      metrics: Arc::new(CommandMetrics::default()),
    }
  }

  pub(crate) async fn acquire(
    &self,
    class: CommandClass,
  ) -> Result<CommandPermit, CommandAdmissionError> {
    match class {
      CommandClass::ActionHost => {
        let active = self
          .action_host_active
          .clone()
          .try_acquire_owned()
          .map_err(|_| CommandAdmissionError::ActionHostBusy)?;
        Ok(CommandPermit::action_host(active, self.metrics.clone()))
      }
      CommandClass::Monitor => {
        let active = self
          .monitor_active
          .clone()
          .try_acquire_owned()
          .map_err(|_| {
            self.metrics.record_monitor_skip();
            CommandAdmissionError::MonitorBusy
          })?;
        Ok(CommandPermit::monitor(
          active,
          self.metrics.clone(),
        ))
      }
      CommandClass::User => {
        let queue = self
          .user_queue
          .clone()
          .try_acquire_owned()
          .map_err(|_| {
            self.metrics.record_user_queue_rejection();
            CommandAdmissionError::UserQueueFull
          })?;
        let queued = QueueMetricGuard::new(self.metrics.clone());
        let active = tokio::time::timeout(
          self.limits.user_wait,
          self.user_active.clone().acquire_owned(),
        )
        .await
        .map_err(|_| {
          self.metrics.record_user_timeout();
          CommandAdmissionError::UserTimeout
        })?
        .map_err(|_| CommandAdmissionError::Closed)?;
        drop(queue);
        drop(queued);
        Ok(CommandPermit::user(
          active,
          self.metrics.clone(),
        ))
      }
    }
  }
}

pub fn command_budget() -> &'static CommandBudget {
  static BUDGET: LazyLock<CommandBudget> = LazyLock::new(|| {
    CommandBudget::new(CommandLimits {
      user_active: USER_COMMAND_PERMITS,
      user_queue: USER_COMMAND_QUEUE,
      user_wait: USER_COMMAND_WAIT,
      action_host_active: ACTION_HOST_COMMAND_PERMITS,
      monitor_active: MONITOR_COMMAND_PERMITS,
    })
  });
  &BUDGET
}
```

Add `thiserror.workspace = true` and `serde.workspace = true` to
`[dependencies]` in `lib/command/Cargo.toml`; the current command crate does
not declare either direct dependency.
`CommandPermit` owns the semaphore permit and an active-metric RAII guard.
`QueueMetricGuard` decrements on admission, timeout, closure, or future
cancellation. Add the explicit imports
`std::sync::atomic::{AtomicU64, AtomicUsize, Ordering}`,
`tokio::sync::OwnedSemaphorePermit`, and `serde::Serialize`. Regenerate
and commit `Cargo.lock` for this direct `thiserror` dependency and Task 5's
direct `libc` dependency.

Expose:

```rust
#[derive(Clone, Debug, Serialize)]
pub struct CommandMetricsSnapshot {
  pub queued_user: usize,
  pub peak_queued_user: usize,
  pub active_user: usize,
  pub peak_user: usize,
  pub active_action_host: usize,
  pub peak_action_host: usize,
  pub active_monitor: usize,
  pub peak_monitor: usize,
  pub rejected_user_queue: u64,
  pub rejected_user_timeout: u64,
  pub skipped_monitor: u64,
}

pub fn command_metrics() -> CommandMetricsSnapshot;
```

Window peaks/counters reset only after a snapshot and gauges remain live.
Reset peaks to the current gauge rather than zero for commands spanning a
window boundary, using a compare-exchange loop that cannot erase a concurrent
new peak.
Tests wait on these gauges instead of scheduler yields.
Re-export `command_metrics`, `CommandMetricsSnapshot`,
`USER_COMMAND_PERMITS`, and `ACTION_HOST_COMMAND_PERMITS` from
`lib/command/src/lib.rs`; keep
`CommandPermit` and mutable counter internals crate-private.

- [ ] **Step 4: Acquire capacity before spawning a child**

Thread `CommandClass` through private command runners. Existing public methods
pass `CommandClass::User`. Add one monitor-only public entry point that keeps
the current `Log` return contract:

```rust
pub async fn run_komodo_standard_monitor_command_with_timeout(
  stage: &str,
  path: impl Into<Option<&Path>>,
  command: impl Into<String>,
  timeout: Duration,
) -> Log {
  let command = command.into();
  let start_ts = komodo_timestamp();
  let output = run_standard_command_inner(
    &command,
    path,
    Some(timeout),
    CommandClass::Monitor,
  )
  .await;
  output_into_log(stage, command, start_ts, output)
}
```

Add a second narrow public wrapper,
`run_komodo_action_host_command(stage, path, command)`, whose body is the
current `run_komodo_standard_command` body but passes
`CommandClass::ActionHost` to `run_standard_command_inner`. Change only
`api/execute/action.rs`'s Deno host-process call to this wrapper. Action hosts
therefore occupy at most six dedicated permits while any command-backed leaf
API they call uses the independent User pool. All other existing public command
runners continue to pass `CommandClass::User`; Periphery monitor polling alone
passes Monitor.

Add a sub-500 ms Core integration regression: six admitted Action roots hold
all six ActionHost permits and concurrently call a command-backed maintenance
leaf. The six maintenance commands acquire User permits and complete; no
ActionHost/User admission timeout occurs and both metric classes return to
zero. This is the required reentrancy proof, not merely a unit semaphore test.

At the start of `run_command_output`, before `cmd.spawn()`, add:

```rust
  let _permit = match command_budget().acquire(class).await {
    Ok(permit) => permit,
    Err(error) => return CommandOutput::from_overload(&error.to_string()),
  };
```

The owned permit stays in scope through child wait and pipe drain.
It is declared before `Child`/`ProcessGroupGuard`, so cancellation drops
the armed group guard before releasing command capacity. Extend Task 5's
cancellation regression to wait for `active_user == 0` after Task 10 metrics
are wired.

In `bin/periphery/src/docker/mod.rs`, replace the raw credential-login
`spawn`, stdin write, and `wait_with_output` sequence with the Task 5
stdin-capable runner. Pass the password as `SecretInput`, use the default
`CommandClass::User`, preserve the existing success/error mapping, and never
format the password. Add an injected login test that holds all six active
permits, fills the 64 queue slots, and asserts the next login returns the
explicit overload output without spawning. After waiter cancellation,
active/queued metrics return to zero. A source assertion requires no
`wait_with_output(` or direct `.spawn()` inside the credential-login function.

- [ ] **Step 5: Classify compose-project polling as monitor work**

In `bin/periphery/src/docker/compose.rs`, replace the command import and call
without changing the surrounding `Log` handling:

```rust
use command::run_komodo_standard_monitor_command_with_timeout;

let res = run_komodo_standard_monitor_command_with_timeout(
  "List Projects",
  None,
  format!("{docker_compose} ls --all --format json"),
  LIST_PROJECTS_COMMAND_TIMEOUT,
)
.await;
```

All user-triggered compose/container/build commands stay on the default user
class. In `docker_lists`, a busy monitor slot produces the existing
empty-project fallback and does not consume a user permit.

- [ ] **Step 6: Poll at most six commands in bulk container handlers**

Replace `join_all` in each of `StartAllContainers`, `RestartAllContainers`,
`PauseAllContainers`, `UnpauseAllContainers`, and `StopAllContainers` with the
same bounded stream expression:

```rust
    let mut results = futures_util::stream::iter(
      futures.into_iter().enumerate().map(|(index, future)| {
        async move { (index, future.await) }
      }),
    )
    .buffer_unordered(command::USER_COMMAND_PERMITS)
    .collect::<Vec<_>>()
    .await;
    results.sort_by_key(|(index, _)| *index);
    Ok(results.into_iter().map(|(_, result)| result).collect())
```

Re-export `USER_COMMAND_PERMITS` from `lib/command/src/lib.rs` and import
`futures_util::StreamExt`; remove the now-unused `join_all` import.

- [ ] **Step 7: Emit Periphery runtime and stats-refresh windows**

Create `bin/periphery/src/runtime_metrics.rs` and start one observer from
`bin/periphery/src/lib.rs`. Instrument the existing stats refresh loop in
`stats.rs` with an RAII duration sample. Every one-second window emits exact
event `periphery_runtime_metrics` with:

- `window_start_unix_ms` and `window_end_unix_ms`;
- nearest-rank `tokio_lag_p99_ms` from absolute 100 ms ticks;
- `tokio_busy_pct` from the delta of every Tokio worker's
  `worker_total_busy_duration`;
- `stats_refresh_count`, `stats_refresh_max_ms`, and
  `stats_refresh_last_completed_unix_ms`;
- the full `CommandMetricsSnapshot`.

Do not report a synthetic zero when no stats refresh completed; report count
zero and omit max/last. The staging selector in Task 15 must include at least
one window with `stats_refresh_count > 0`.

- [ ] **Step 8: Verify reserved capacity, metrics, and process bounds**

Run: `rtk cargo test -p command budget::tests`

Expected: budget, cancellation, metrics, and the 10/100/1,000 matrix pass;
`peak_user <= 6`, `peak_action_host <= 6`, `peak_monitor <= 2`,
`peak_queued_user <= 64`, and
gauges return to zero.

Run: `rtk cargo test -p command --test bounded_output cancelling_command_future_kills_background_descendant -- --nocapture`

Expected: the descendant is stopped and the command active gauge reaches zero
before the test deadline.

Run: `rtk cargo test -p komodo_periphery`

Expected: all Periphery tests pass.

Run: `rtk cargo test -p komodo_core action_host_command_reentrancy -- --nocapture`

Expected: six Action hosts and their six command-backed leaf calls complete
inside the watchdog with independent gauges.

Run: `rtk cargo check -p komodo_core -p komodo_periphery`

Expected: all command call sites compile with the workload class threaded through private runners.

Run:

```bash
rtk proxy sh -c 'if rtk rg -n "wait_with_output\\(|\.spawn\\(\)" bin/periphery/src/docker/mod.rs; then exit 1; fi'
```

Expected: the credential-login path has no budget/output bypass.

- [ ] **Step 9: Commit and open checkpoint 3**

```bash
rtk git add Cargo.lock bin/core/src/runtime.rs bin/core/src/lib.rs bin/core/src/api/execute/dispatch.rs bin/core/src/api/execute/mod.rs bin/core/src/api/execute/action.rs bin/core/src/helpers/procedure_tree.rs bin/core/src/helpers/procedure.rs bin/core/src/monitor/mod.rs bin/core/src/monitor/swarm.rs lib/command/Cargo.toml lib/command/src/lib.rs lib/command/src/budget.rs lib/command/tests/bounded_output.rs bin/periphery/src/docker/mod.rs bin/periphery/src/docker/compose.rs bin/periphery/src/api/container/mod.rs bin/periphery/src/runtime_metrics.rs bin/periphery/src/stats.rs bin/periphery/src/lib.rs
rtk git commit -m "perf: separate child process capacity"
rtk git push -u origin runtime-work-budgets
rtk gh pr create --repo intezya/komodo --base main --head runtime-work-budgets --title "Enforce runtime work budgets" --body "Routes API, schedule, startup, webhook, and sync executions through cancellation-safe Core admission; separates 32 leaf/6 orchestrator/8 Procedure-root permits; bounds non-recursive Procedure trees; isolates Action-host, user, and monitor command capacity. Verification: cargo test -p komodo_core; cargo test -p command; cargo test -p komodo_periphery."
```

Expected: fork-only PR created; batch tests at 10, 100, and 1,000 show active permit counts never exceed the constants above.

### Task 11: Enforce one 8 MiB persisted Update log budget

**Files:**
- Modify: `bin/core/src/helpers/update.rs:1-130`
- Modify: `bin/core/src/api/execute/action.rs`
- Modify: `bin/core/src/api/execute/build.rs`
- Modify: `bin/core/src/api/execute/repo.rs`
- Modify: `bin/core/src/helpers/procedure_tree.rs`

- [ ] **Step 1: Write failing UTF-8 and whole-Update cap tests**

Add to the existing `bin/core/src/helpers/update.rs` test module:

```rust
#[test]
fn update_logs_are_capped_once_with_terminal_headroom() {
  let mut update = Update::default();
  update.logs = vec![
    Log::simple("first", "a".repeat(6 * 1024 * 1024)),
    Log::error("second", "b".repeat(4 * 1024 * 1024)),
  ];

  enforce_update_log_budget(&mut update);

  assert!(update_log_bytes(&update) <= UPDATE_LOG_LIMIT_BYTES);
  let combined = update
    .logs
    .iter()
    .flat_map(|log| [&log.stdout, &log.stderr])
    .cloned()
    .collect::<String>();
  assert_eq!(combined.matches(UPDATE_LOG_TRUNCATION_MARKER).count(), 1);
}

#[test]
fn update_log_cap_preserves_valid_utf8() {
  let mut update = Update::default();
  update.logs = vec![Log::simple(
    "unicode",
    "🦎".repeat(3 * 1024 * 1024),
  )];

  enforce_update_log_budget(&mut update);

  assert!(update.logs[0].stdout.is_char_boundary(
    update.logs[0].stdout.len()
  ));
  assert!(update_log_bytes(&update) <= UPDATE_LOG_LIMIT_BYTES);
}

#[test]
fn update_log_cap_is_idempotent_and_preserves_terminal_error() {
  let mut update = Update::default();
  update.status = UpdateStatus::Complete;
  update.logs = vec![
    Log::simple("progress", "x".repeat(10 * 1024 * 1024)),
    Log::error("execution error", "terminal failure".into()),
  ];

  enforce_update_log_budget(&mut update);
  enforce_update_log_budget(&mut update);

  assert!(update_log_bytes(&update) <= UPDATE_LOG_LIMIT_BYTES);
  assert!(update.logs[1].stderr.contains("terminal failure"));
  let marker_count = update
    .logs
    .iter()
    .flat_map(|log| {
      [&log.stage, &log.command, &log.stdout, &log.stderr]
    })
    .map(|value| value.matches(UPDATE_LOG_TRUNCATION_MARKER).count())
    .sum::<usize>();
  assert_eq!(marker_count, 1);
}

#[test]
fn single_terminal_error_log_receives_terminal_budget() {
  let mut update = Update::default();
  update.status = UpdateStatus::Complete;
  update.logs = vec![Log::error(
    "execution error",
    format!("actual terminal error: {}", "x".repeat(10 * 1024 * 1024)),
  )];

  enforce_update_log_budget(&mut update);

  assert!(update.logs[0].stderr.starts_with("actual terminal error"));
  assert!(update_log_bytes(&update) <= UPDATE_LOG_LIMIT_BYTES);
}

#[test]
fn terminal_stderr_has_priority_over_large_terminal_stdout() {
  let mut update = Update::default();
  update.status = UpdateStatus::Complete;
  let mut terminal = Log::error(
    "execution error",
    "actual terminal failure".to_string(),
  );
  terminal.stdout = "noise".repeat(2 * 1024 * 1024);
  update.logs = vec![
    Log::simple("progress", "x".repeat(8 * 1024 * 1024)),
    terminal,
  ];

  enforce_update_log_budget(&mut update);

  assert!(update.logs[1].stderr.contains("actual terminal failure"));
  assert!(update_log_bytes(&update) <= UPDATE_LOG_LIMIT_BYTES);
}
```

- [ ] **Step 2: Run tests and verify RED**

Run: `rtk cargo test -p komodo_core helpers::update::tests::update_log -- --nocapture`

Expected: compilation fails because log-budget functions and constants are missing.

- [ ] **Step 3: Implement deterministic whole-Update truncation**

Add `Log` to the existing `entities::update::{...}` import, then add near the
top of `bin/core/src/helpers/update.rs`:

```rust
pub const UPDATE_LOG_LIMIT_BYTES: usize = 8 * 1024 * 1024;
pub const UPDATE_TERMINAL_LOG_RESERVE_BYTES: usize = 64 * 1024;
pub const UPDATE_LOG_TRUNCATION_MARKER: &str =
  "\n[komodo: update log truncated at 8 MiB]\n";

fn log_string_bytes(log: &Log) -> usize {
  log
    .stage
    .len()
    .saturating_add(log.command.len())
    .saturating_add(log.stdout.len())
    .saturating_add(log.stderr.len())
}

pub fn update_log_bytes(update: &Update) -> usize {
  update.logs.iter().fold(0, |total, log| {
    total.saturating_add(log_string_bytes(log))
  })
}

fn truncate_string_to_bytes(value: &mut String, max: usize) {
  let mut end = max.min(value.len());
  while !value.is_char_boundary(end) {
    end -= 1;
  }
  value.truncate(end);
}

fn strip_existing_markers(update: &mut Update) -> bool {
  let mut found = false;
  for log in &mut update.logs {
    for value in [
      &mut log.stage,
      &mut log.command,
      &mut log.stdout,
      &mut log.stderr,
    ] {
      if value.contains(UPDATE_LOG_TRUNCATION_MARKER) {
        *value = value.replace(UPDATE_LOG_TRUNCATION_MARKER, "");
        found = true;
      }
    }
  }
  found
}

fn truncate_log(log: &mut Log, remaining: &mut usize) {
  for value in [
    &mut log.stage,
    &mut log.command,
    &mut log.stdout,
    &mut log.stderr,
  ] {
    if value.len() <= *remaining {
      *remaining -= value.len();
    } else {
      truncate_string_to_bytes(value, *remaining);
      *remaining = 0;
    }
  }
}

fn truncate_terminal_log(
  log: &mut Log,
  remaining: &mut usize,
) {
  // Preserve the error-bearing fields before potentially noisy stdout.
  for value in [
    &mut log.stage,
    &mut log.command,
    &mut log.stderr,
    &mut log.stdout,
  ] {
    if value.len() <= *remaining {
      *remaining -= value.len();
    } else {
      truncate_string_to_bytes(value, *remaining);
      *remaining = 0;
    }
  }
}

pub fn enforce_update_log_budget(update: &mut Update) {
  let had_marker = strip_existing_markers(update);
  let raw_bytes = update_log_bytes(update);
  if raw_bytes <= UPDATE_LOG_LIMIT_BYTES && !had_marker {
    return;
  }

  let payload_limit = UPDATE_LOG_LIMIT_BYTES
    .saturating_sub(UPDATE_LOG_TRUNCATION_MARKER.len());
  let terminal_index = (update.status == UpdateStatus::Complete
    && !update.logs.is_empty())
    .then(|| update.logs.len() - 1);
  let head_bytes = update
    .logs
    .iter()
    .enumerate()
    .filter(|(index, _)| Some(*index) != terminal_index)
    .fold(0_usize, |total, (_, log)| {
      total.saturating_add(log_string_bytes(log))
    });
  let terminal_bytes = terminal_index
    .map(|index| log_string_bytes(&update.logs[index]))
    .unwrap_or_default();
  let terminal_budget = terminal_bytes
    .min(
      UPDATE_TERMINAL_LOG_RESERVE_BYTES.max(
        payload_limit.saturating_sub(head_bytes),
      ),
    )
    .min(payload_limit);
  let mut head_remaining = payload_limit - terminal_budget;
  for (index, log) in update.logs.iter_mut().enumerate() {
    if Some(index) != terminal_index {
      truncate_log(log, &mut head_remaining);
    }
  }
  if let Some(index) = terminal_index {
    let mut terminal_remaining = terminal_budget;
    truncate_terminal_log(
      &mut update.logs[index],
      &mut terminal_remaining,
    );
  }
  if let Some(first) = update.logs.first_mut() {
    first.stdout.push_str(UPDATE_LOG_TRUNCATION_MARKER);
  }
}
```

The budget covers every persisted string in every `Log`, not only stdout and
stderr; a user-supplied command or stage therefore cannot bypass the 8 MiB
document headroom. Existing markers are normalized before every pass, making
the function idempotent. For any completed Update with at least one Log, at
least 64 KiB of the payload budget is reserved for its last terminal Log
before older progress is truncated. Within that terminal budget,
stage/command/error stderr are retained before noisy stdout, so a large
success stream cannot erase the actual terminal failure.

Make one helper own every full-document persistence boundary:

```rust
pub async fn persist_update(
  update: &mut Update,
) -> anyhow::Result<()> {
  enforce_update_log_budget(update);
  update_one_by_id(
    &db_client().updates,
    &update.id,
    database::mungos::update::Update::Set(to_document(&*update)?),
    None,
  )
  .await
  .context(
    "failed to update the update on db. the update process was deleted",
  )?;
  Ok(())
}

pub async fn broadcast_update(
  update: Update,
) -> anyhow::Result<()> {
  let update = update_list_item(update).await?;
  send_update(update).await
}

pub async fn update_update(
  mut update: Update,
) -> anyhow::Result<()> {
  persist_update(&mut update).await?;
  let _ = broadcast_update(update).await;
  Ok(())
}
```

Call `enforce_update_log_budget` before the insert in `add_update`; clone the
borrowed value and enforce it before the insert in
`add_update_without_send`. Keep `update_update` as the general
persist-then-broadcast path.

- [ ] **Step 4: Route every direct full replacement through the cap**

In `api/execute/{action,build,repo}.rs` and Task 9's Procedure terminal path
in `helpers/procedure_tree.rs`, replace every manual
`to_document(&update)` plus `Update::Set` block with this ordering:

```rust
persist_update(&mut update).await?;
refresh_build_state_cache().await;
broadcast_update(update.clone()).await?;
```

Change the existing immutable `let update = ...` bindings in
`api/execute/action.rs` and `api/execute/repo.rs` to `let mut update = ...`
before passing them to `persist_update(&mut update)`; keep already-mutable
bindings in Build and the scheduler finalizer unchanged. Import
`persist_update` and `broadcast_update` from `crate::helpers::update` in
every converted module.

Use the resource-specific refresh function in each module. These blocks
currently persist once before refreshing and then call `update_update`, which
persists the same document a second time. Remove that trailing
`update_update(update.clone()).await?` from each converted block. The new
sequence remains database → state cache → broadcast, but performs one capped
write. Update imports to use `persist_update` and `broadcast_update`; remove
now-unused `to_document`, `update_one_by_id`, and `update_update` imports only
where the final use disappears.

Prove the repository has no bypass:

```bash
rtk rg -n 'Update::Set|to_document\(&update\)' \
  bin/core/src/api/execute \
  bin/core/src/helpers/procedure_tree.rs
```

Expected: no matches. Targeted `$set`, `$push`, and pipeline delta writes are
allowed because they are not full-Update replacements and are bounded
separately in Task 12.

- [ ] **Step 5: Verify caps and existing Update behavior**

Run: `rtk cargo test -p komodo_core helpers::update::tests -- --nocapture`

Expected: new cap tests and all existing permission/update tests pass.

Run:

```bash
rtk cargo test -p komodo_core api::execute::action
rtk cargo test -p komodo_core api::execute::build
rtk cargo test -p komodo_core api::execute::repo
rtk cargo test -p komodo_core helpers::procedure_tree::tests
```

Expected: Action, Build, Repo, and Procedure-tree finalization pass with the
single-write ordering.

- [ ] **Step 6: Commit the storage safety boundary**

```bash
rtk git add bin/core/src/helpers/update.rs bin/core/src/api/execute/action.rs bin/core/src/api/execute/build.rs bin/core/src/api/execute/repo.rs bin/core/src/helpers/procedure_tree.rs
rtk git commit -m "fix: cap persisted update logs"
```

### Task 12: Replace procedure whole-document progress writes with bounded delta batches

**Files:**
- Create: `bin/core/src/helpers/update_stream.rs`
- Modify: `bin/core/src/helpers/mod.rs:20-45`
- Modify: `bin/core/src/helpers/update.rs:80-130`
- Modify: `bin/core/src/helpers/procedure.rs:30-225,510-540`
- Modify: `bin/core/src/helpers/procedure_tree.rs`
- Modify: `bin/core/src/api/execute/procedure.rs:80-175`

- [ ] **Step 1: Write failing buffer and flush-policy tests**

Create `bin/core/src/helpers/update_stream.rs` with these tests first:

```rust
#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn progress_buffer_emits_chunks_no_larger_than_sixty_four_kib() {
    let mut buffer = ProgressBuffer::new(0);
    buffer.push(&"x".repeat(200 * 1024), false);
    let mut total = 0;
    while let Some(chunk) = buffer.take_chunk(true) {
      assert!(chunk.len() <= UPDATE_APPEND_BATCH_BYTES);
      total += chunk.len();
    }
    assert_eq!(total, 200 * 1024);
  }

  #[test]
  fn progress_buffer_marks_truncation_once_and_discards_the_rest() {
    let mut buffer = ProgressBuffer::new(0);
    buffer.push(&"x".repeat(10 * 1024 * 1024), false);
    buffer.push(&"y".repeat(10 * 1024 * 1024), false);
    let mut persisted = String::new();
    while let Some(chunk) = buffer.take_chunk(true) {
      persisted.push_str(&chunk);
    }
    assert!(persisted.len() <= UPDATE_LOG_LIMIT_BYTES);
    assert_eq!(persisted.matches(UPDATE_LOG_TRUNCATION_MARKER).count(), 1);
  }

  #[test]
  fn writer_task_error_survives_a_dropped_finish_ack() {
    let error = combine_finish_results(
      Err(anyhow::anyhow!("finish acknowledgement dropped")),
      Err(anyhow::anyhow!("mongo append failed")),
    )
    .unwrap_err();
    assert!(error.to_string().contains("mongo append failed"));
  }
}
```

- [ ] **Step 2: Run tests and verify RED**

Register `pub mod update_stream;` in `bin/core/src/helpers/mod.rs`, then run:

Run: `rtk cargo test -p komodo_core helpers::update_stream::tests -- --nocapture`

Expected: compilation fails because `ProgressBuffer` is missing.

- [ ] **Step 3: Implement the bounded in-memory buffer**

Add above the tests:

```rust
use std::{sync::Arc, time::Duration};

use anyhow::Context;
use database::mungos::mongodb::{
  bson::{doc, oid::ObjectId},
  options::UpdateModifications,
};
use komodo_client::entities::update::Update;
use tokio::{
  sync::{
    Mutex, OwnedSemaphorePermit, Semaphore, mpsc, oneshot,
  },
  task::JoinHandle,
};

use crate::{
  helpers::update::{
    UPDATE_LOG_LIMIT_BYTES, UPDATE_LOG_TRUNCATION_MARKER,
    UPDATE_TERMINAL_LOG_RESERVE_BYTES, send_update,
    update_list_item, update_log_bytes,
  },
  state::db_client,
};

pub const UPDATE_APPEND_BATCH_BYTES: usize = 64 * 1024;
pub const UPDATE_APPEND_INTERVAL: Duration = Duration::from_millis(250);
const PROGRESS_CHANNEL_CAPACITY: usize = 256;
const PROGRESS_QUEUED_BYTES: usize = UPDATE_LOG_LIMIT_BYTES;

struct ProgressBuffer {
  pending: String,
  offset: usize,
  accepted: usize,
  payload_limit: usize,
  truncated: bool,
}

impl ProgressBuffer {
  fn new(existing_bytes: usize) -> Self {
    Self {
      pending: String::new(),
      offset: 0,
      accepted: existing_bytes,
      payload_limit: UPDATE_LOG_LIMIT_BYTES
        .saturating_sub(UPDATE_TERMINAL_LOG_RESERVE_BYTES)
        .saturating_sub(UPDATE_LOG_TRUNCATION_MARKER.len()),
      truncated: false,
    }
  }

  fn push(
    &mut self,
    input: &str,
    source_truncated: bool,
  ) -> String {
    if self.truncated {
      return String::new();
    }
    let available = self.payload_limit.saturating_sub(self.accepted);
    let mut end = available.min(input.len());
    while !input.is_char_boundary(end) {
      end -= 1;
    }
    let mut accepted = input[..end].to_string();
    self.accepted += accepted.len();
    if end < input.len() || source_truncated {
      accepted.push_str(UPDATE_LOG_TRUNCATION_MARKER);
      self.truncated = true;
    }
    self.pending.push_str(&accepted);
    accepted
  }

  fn pending_bytes(&self) -> usize {
    self.pending.len().saturating_sub(self.offset)
  }

  fn take_chunk(&mut self, flush_partial: bool) -> Option<String> {
    if self.offset == self.pending.len() {
      self.pending.clear();
      self.offset = 0;
      return None;
    }
    if !flush_partial && self.pending_bytes() < UPDATE_APPEND_BATCH_BYTES {
      return None;
    }
    let mut end = (self.offset + UPDATE_APPEND_BATCH_BYTES)
      .min(self.pending.len());
    while !self.pending.is_char_boundary(end) {
      end -= 1;
    }
    let chunk = self.pending[self.offset..end].to_string();
    self.offset = end;
    Some(chunk)
  }
}
```

- [ ] **Step 4: Add an atomic Mongo delta append**

Add this function in the same file:

```rust
async fn append_first_log_stdout(
  update_id: &str,
  chunk: &str,
) -> anyhow::Result<()> {
  let id = ObjectId::parse_str(update_id)
    .context("update id is not an object id")?;
  let result = db_client()
    .updates
    .update_one(
      doc! { "_id": id },
      UpdateModifications::Pipeline(vec![doc! {
        "$set": {
          "logs.0.stdout": {
            "$concat": [
              { "$ifNull": ["$logs.0.stdout", ""] },
              chunk,
            ]
          }
        }
      }]),
    )
    .await
    .context("failed to append procedure progress")?;
  if result.matched_count != 1 {
    anyhow::bail!("procedure Update disappeared before progress append");
  }
  Ok(())
}
```

There is one writer per procedure Update, so chunks remain ordered. Do not add
automatic retries after an ambiguous Mongo network result because a retry can
duplicate a committed chunk.

- [ ] **Step 5: Implement the single writer and periodic coalescing loop**

Add to `update_stream.rs`:

```rust
enum ProgressMessage {
  Line {
    line: String,
    source_truncated: bool,
    _queued_bytes: OwnedSemaphorePermit,
  },
  Finish(oneshot::Sender<()>),
}

enum FlushReason {
  FullBatch,
  Timer,
  Finish(Option<oneshot::Sender<()>>),
}

#[derive(Clone)]
pub struct ProcedureProgressSink {
  sender: mpsc::Sender<ProgressMessage>,
  queued_bytes: Arc<Semaphore>,
}

pub struct ProcedureProgressWriter {
  sink: ProcedureProgressSink,
  task: JoinHandle<anyhow::Result<()>>,
}

fn combine_finish_results(
  signal_result: anyhow::Result<()>,
  task_result: anyhow::Result<()>,
) -> anyhow::Result<()> {
  match (signal_result, task_result) {
    (_, Err(task_error)) => Err(task_error.context(
      "procedure progress writer failed before finish",
    )),
    (Err(signal_error), Ok(())) => Err(signal_error),
    (Ok(()), Ok(())) => Ok(()),
  }
}

impl ProcedureProgressWriter {
  pub async fn start(
    update: Arc<Mutex<Update>>,
  ) -> anyhow::Result<Self> {
    let initial = update.lock().await.clone();
    let existing_bytes = update_log_bytes(&initial);
    let list_item = update_list_item(initial).await?;
    let (sender, mut receiver) =
      mpsc::channel(PROGRESS_CHANNEL_CAPACITY);
    let queued_bytes =
      Arc::new(Semaphore::new(PROGRESS_QUEUED_BYTES));
    let task = tokio::spawn(async move {
      let mut buffer = ProgressBuffer::new(existing_bytes);
      let mut interval = tokio::time::interval(UPDATE_APPEND_INTERVAL);
      interval.set_missed_tick_behavior(
        tokio::time::MissedTickBehavior::Skip,
      );
      interval.tick().await;

      loop {
        let reason = tokio::select! {
          message = receiver.recv() => match message {
            Some(ProgressMessage::Line {
              line,
              source_truncated,
              _queued_bytes,
            }) => {
              let accepted = buffer.push(&line, source_truncated);
              if !accepted.is_empty() {
                update.lock().await.logs[0].stdout.push_str(&accepted);
              }
              FlushReason::FullBatch
            }
            Some(ProgressMessage::Finish(done)) => {
              FlushReason::Finish(Some(done))
            }
            None => FlushReason::Finish(None),
          },
          _ = interval.tick() => FlushReason::Timer,
        };

        let flush_partial = !matches!(reason, FlushReason::FullBatch);
        let mut flushed = false;
        while let Some(chunk) = buffer.take_chunk(flush_partial) {
          append_first_log_stdout(&list_item.id, &chunk).await?;
          flushed = true;
        }
        if flushed {
          send_update(list_item.clone()).await?;
        }

        if let FlushReason::Finish(done) = reason {
          if let Some(done) = done {
            let _ = done.send(());
          }
          break;
        }
      }
      Ok(())
    });
    Ok(Self {
      sink: ProcedureProgressSink {
        sender,
        queued_bytes,
      },
      task,
    })
  }

  pub fn sink(&self) -> ProcedureProgressSink {
    self.sink.clone()
  }

  pub async fn finish(self) -> anyhow::Result<()> {
    let Self { sink, task } = self;
    let ProcedureProgressSink {
      sender,
      queued_bytes: _,
    } = sink;
    let (done, received) = oneshot::channel();
    let signal_result = async {
      sender
        .send(ProgressMessage::Finish(done))
        .await
        .context("procedure progress writer closed before finish")?;
      received
        .await
        .context("procedure progress finish acknowledgement dropped")
    }
    .await;
    drop(sender);
    let task_result = task
      .await
      .context("procedure progress task panicked")?;
    combine_finish_results(signal_result, task_result)
  }
}

impl ProcedureProgressSink {
  pub async fn push_line(&self, line: String) -> anyhow::Result<()> {
    let mut line = format!("\n{line}");
    let source_truncated = line.len() > UPDATE_LOG_LIMIT_BYTES;
    if source_truncated {
      let mut end = UPDATE_LOG_LIMIT_BYTES;
      while !line.is_char_boundary(end) {
        end -= 1;
      }
      line.truncate(end);
    }
    let queued_bytes = self
      .queued_bytes
      .clone()
      .acquire_many_owned(line.len() as u32)
      .await
      .context("procedure progress byte budget closed")?;
    self
      .sender
      .send(ProgressMessage::Line {
        line,
        source_truncated,
        _queued_bytes: queued_bytes,
      })
      .await
      .context("procedure progress writer closed")
  }
}
```

The message count and byte semaphore jointly bound queued progress to 256
messages and 8 MiB. Each flush cycle emits at most one progress event even
when it needs several 64 KiB Mongo appends, and progress never uses
`update_update`. The terminal writer remains separate.

- [ ] **Step 6: Expose only the two existing Update helpers needed by the writer**

In `bin/core/src/helpers/update.rs`, change only these visibilities:

```rust
pub(super) async fn update_list_item(
  update: Update,
) -> anyhow::Result<UpdateListItem> {
```

```rust
pub(super) async fn send_update(
  update: UpdateListItem,
) -> anyhow::Result<()> {
```

- [ ] **Step 7: Integrate the writer with the bounded Procedure tree**

Task 9 has already removed the recursive `execute_procedure_stage` lifecycle.
Do not recreate it or thread a writer through obsolete recursive functions.
Instead, extend `ProcedureRootRequest`, coordinator node state, expansion
jobs, leaf/Action jobs, and node-finalizer jobs in
`helpers/procedure_tree.rs`.

When the dispatcher or coordinator initializes a Procedure node, push its
initial progress Log, persist **and broadcast** the InProgress Update, and
create:

```rust
struct ProcedureNodeProgress {
  update: Arc<Mutex<Update>>,
  writer: Option<ProcedureProgressWriter>,
  sink: ProcedureProgressSink,
}
```

The writer owner stays only in coordinator node state. Every expansion,
ordinary leaf, Action, CommitSync, and Sleep job receives only a cloned
`ProcedureProgressSink`. Replace the old `add_line_to_update` sites with
`sink.push_line(formatted_line).await?` for stage start/end, child
start/completion, and errors; then delete `add_line_to_update`. Worker result
messages contain outcome/timing only and must drop their sink before sending
the result.

Add a `FinalizeNode` scheduler job. A node becomes finalizable only when its
ready jobs are removed, every assigned worker result has returned, and every
required child node is terminal. The coordinator then moves the whole
`ProcedureNodeProgress` into the finalizer; no copy remains in its root map.
The finalizer performs this exact order:

1. drop the node's last coordinator-owned `ProcedureProgressSink`;
2. take and `finish().await` the writer, which flushes queued lines and joins
   its task even when the finish send/ack fails;
3. `Arc::try_unwrap(update)`; failure is a scheduler invariant error naming
   the node/root IDs, never a retry loop;
4. combine execution and writer errors without losing either chain;
5. append the normal success/error terminal Log and call `finalize()`;
6. `persist_update(&mut update).await`, refresh the Procedure state cache,
   and `broadcast_update(update.clone()).await`;
7. return `NodeFinalized` so the coordinator may advance the parent stage or
   complete the root oneshot.

Run finalizers on the existing 32-worker tree job pool without a leaf/orchestrator
runtime permit; Mongo/writer completion is already bounded by at most 32
workers and eight roots. On cancellation or scheduler overload, stop assigning
new execution jobs, cancel the root token, clear ready execution descriptors
(dropping their sinks), drain the bounded `ChildCancelled`/completion results
from every assigned worker, then schedule finalizers leaf-to-root. Every worker
uses Task 9's cancellation-vs-work `select!`; finalization must not wait for the
cancelled work future itself. Keep
a reserved direct-to-idle-worker finalization lane so terminal persistence
cannot be rejected by the already-triggered 256-ready-job overload. Idle
workers always take reserved finalizers before ordinary ready jobs.

Nested Procedure nodes use their own Update/writer/sink while their parent gets
only child start/completion lines. The dispatch task's
`ProcedureRootPermit` remains owned until its root `NodeFinalized` result.
This preserves child Update events and ensures no parent-held worker or writer
Arc survives root completion.

In `api/execute/procedure.rs`, remove the pre-Task-9 local
`execute_procedure`/mutex/finalization block. The root resolver delegates to
`ProcedureTreeScheduler::submit`; node finalizers now own terminal persistence
for both root and nested Procedure Updates. Import
`ProcedureProgressWriter`/`ProcedureProgressSink` where constructed and
retain `persist_update`/`broadcast_update` only in the finalizer module.

- [ ] **Step 8: Verify progress batching and procedure behavior**

Run: `rtk cargo test -p komodo_core helpers::update_stream::tests -- --nocapture`

Expected: buffer/writer tests pass, including a cloned sink that is dropped
before owner finish.

Run:

```bash
rtk cargo test -p komodo_core helpers::update::tests
rtk cargo test -p komodo_core api::execute::procedure
rtk cargo test -p komodo_core helpers::procedure_tree::tests -- --nocapture
```

Add and pass these tree/writer regressions:

- a branching tree emits stage/child lines through its node sink and persists
  exactly one terminal event per node;
- cancellation while a delayed leaf owns a sink waits for the result, drops
  every sink, finishes the writer, and makes `Arc::try_unwrap` succeed;
- simultaneous tree-execution and Mongo-writer errors both appear in the
  terminal error chain;
- nested-node finalizers complete leaf-to-root and the root permit is released
  only after the root terminal broadcast acknowledgement.

- [ ] **Step 9: Commit delta progress writes**

```bash
rtk git add bin/core/src/helpers/update_stream.rs bin/core/src/helpers/mod.rs bin/core/src/helpers/update.rs bin/core/src/helpers/procedure.rs bin/core/src/helpers/procedure_tree.rs bin/core/src/api/execute/procedure.rs
rtk git commit -m "perf: batch procedure update progress"
```

### Task 13: Add optional connection epochs and visible-event sequences

**Files:**
- Modify: `client/core/rs/src/entities/update.rs:110-150`
- Modify: `client/core/ts/src/types.ts`
- Modify: `bin/core/src/helpers/update.rs:1-130`
- Modify: `bin/core/src/api/read/update.rs:60-80`

- [ ] **Step 1: Write failing old/new JSON compatibility tests**

Add to `client/core/rs/src/entities/update.rs`:

```rust
#[cfg(test)]
mod event_compatibility_tests {
  use serde::Deserialize;

  use super::*;

  #[derive(Deserialize)]
  struct LegacyUpdateEvent {
    id: String,
  }

  #[test]
  fn new_client_accepts_event_without_stream_metadata() {
    let value = serde_json::to_value(UpdateListItem {
      id: "update-1".to_string(),
      operation: Default::default(),
      start_ts: 0,
      success: true,
      username: "user".to_string(),
      operator: "user-id".to_string(),
      target: Default::default(),
      status: Default::default(),
      version: Default::default(),
      other_data: String::new(),
      stream_epoch: None,
      sequence: None,
    })
    .unwrap();
    let parsed: UpdateListItem = serde_json::from_value(value).unwrap();
    assert_eq!(parsed.stream_epoch, None);
    assert_eq!(parsed.sequence, None);
  }

  #[test]
  fn old_client_shape_ignores_optional_stream_metadata() {
    let value = serde_json::json!({
      "id": "update-1",
      "stream_epoch": "epoch-a",
      "sequence": 42
    });
    let parsed: LegacyUpdateEvent = serde_json::from_value(value).unwrap();
    assert_eq!(parsed.id, "update-1");
  }
}
```

- [ ] **Step 2: Run the compatibility tests and verify RED**

Run: `rtk cargo test -p komodo_client event_compatibility_tests`

Expected: compilation fails because `stream_epoch` and `sequence` are absent.

- [ ] **Step 3: Add optional fields without changing the envelope**

Append to `UpdateListItem`:

```rust
  /// Identifies one authenticated WebSocket connection's visible stream.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub stream_epoch: Option<String>,
  /// Strictly increases within one stream epoch.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub sequence: Option<U64>,
```

Import `U64`. Add `stream_epoch: None, sequence: None` to
both `update_list_item` in Core and the `UpdateListItem` constructor in
`api/read/update.rs` so ordinary list responses remain metadata-free.

- [ ] **Step 4: Add a connection-local visible-event sequencer**

Add in `bin/core/src/helpers/update.rs`:

```rust
pub(crate) struct UpdateStreamSequencer {
  epoch: String,
  next_sequence: u64,
}

impl UpdateStreamSequencer {
  pub fn new(epoch: String) -> Self {
    Self {
      epoch,
      next_sequence: 1,
    }
  }

  pub fn stamp(
    &mut self,
    mut update: UpdateListItem,
  ) -> anyhow::Result<UpdateListItem> {
    let sequence = self.next_sequence;
    self.next_sequence = self
      .next_sequence
      .checked_add(1)
      .context("update websocket sequence overflow")?;
    update.stream_epoch = Some(self.epoch.clone());
    update.sequence = Some(sequence);
    Ok(update)
  }
}
```

Import `anyhow::Context`. Do not call the sequencer from `send_update`:
authorization still happens after the internal broadcast, and globally stamped
events would create false gaps whenever a user is not authorized to see an
intervening event. Task 14 creates one sequencer per authenticated WebSocket and
stamps only events that pass that connection's permission filter. List API
responses and the internal broadcast remain metadata-free.

- [ ] **Step 5: Add deterministic sequencer invariants to Core tests**

Add:

```rust
fn test_list_item(id: &str) -> UpdateListItem {
  UpdateListItem {
    id: id.to_string(),
    operation: Default::default(),
    start_ts: 0,
    success: true,
    username: "user".to_string(),
    operator: "user-id".to_string(),
    target: Default::default(),
    status: Default::default(),
    version: Default::default(),
    other_data: String::new(),
    stream_epoch: None,
    sequence: None,
  }
}

#[test]
fn visible_events_share_epoch_and_have_contiguous_sequences() {
  let mut sequencer =
    UpdateStreamSequencer::new("connection-a".to_string());
  let first = sequencer.stamp(test_list_item("one")).unwrap();
  let second = sequencer.stamp(test_list_item("two")).unwrap();
  assert_eq!(first.stream_epoch, second.stream_epoch);
  assert_eq!(first.stream_epoch.as_deref(), Some("connection-a"));
  assert_eq!(second.sequence, first.sequence.map(|value| value + 1));
}
```

- [ ] **Step 6: Regenerate and build the TypeScript client**

Run: `rtk node client/core/ts/generate_types.mjs`

Expected: `client/core/ts/src/types.ts` contains optional `stream_epoch` and `sequence` fields on `UpdateListItem`.

Run: `rtk yarn --cwd client/core/ts build`

Expected: TypeScript client build succeeds.

- [ ] **Step 7: Verify Rust compatibility and commit**

Run: `rtk cargo test -p komodo_client event_compatibility_tests`

Expected: 2 tests pass.

Run: `rtk cargo test -p komodo_core helpers::update::tests::visible_events_share_epoch_and_have_contiguous_sequences`

Expected: 1 test passes.

```bash
rtk git add client/core/rs/src/entities/update.rs client/core/ts/src/types.ts bin/core/src/helpers/update.rs bin/core/src/api/read/update.rs
rtk git commit -m "feat: sequence update websocket events"
```

### Task 14: Authorize once per user in one ordered Update fan-out hub

**Files:**
- Modify: `bin/core/src/helpers/channel.rs`
- Modify: `bin/core/src/helpers/update.rs`
- Modify: `bin/core/src/api/ws/update.rs`

**Dependency:** The fork's `main` must already contain Plan 1's
`PermissionSnapshotProvider` contract from the gate at the top of this plan.

- [ ] **Step 1: Write RED tests for ordered shared authorization**

Add injected-authorizer tests in `helpers/channel.rs`. A test hub uses
capacity-four event and connection queues so overload is deterministic. Cover:

- one and 32 simultaneous connections for `user-a`: publishing one event
  invokes the authorizer exactly once in both cases;
- 32 distinct users: one event invokes it exactly 32 times;
- two events for one user: authorization is invoked twice, and an allow on the
  first event is not reused after the injected provider returns deny;
- events `one, two, three` arrive in that order at every authorized
  connection;
- a full connection queue cancels and unregisters that connection; it does not
  drop an event while leaving the socket open;
- an authorization error cancels all connections for that user but does not
  prevent other user groups from receiving the event;
- an acknowledged `RefreshUser` before Publish changes the next decision
  without replacing the connection queue or sequence;
- a terminal event immediately after progress is delivered as the next event,
  never coalesced.

Run: `rtk cargo test -p komodo_core helpers::channel::tests -- --nocapture`

Expected: RED because `UpdateFanoutHub` and connection registration do not
exist.

- [ ] **Step 2: Replace the Update broadcast with one serialized hub**

Keep the existing generic `BroadcastChannel` only for build/repo cancellation.
Replace `update_channel()` with a process-wide `update_fanout_hub()` backed
by one bounded command channel:

```rust
pub const UPDATE_HUB_QUEUE: usize = 256;
pub const UPDATE_CONNECTION_QUEUE: usize = 100;
pub const UPDATE_AUTH_CONCURRENCY: usize = 16;

pub struct UpdateFanoutHub;

impl UpdateFanoutHub {
  pub async fn register(
    &self,
    user: User,
    cancel: CancellationToken,
  ) -> anyhow::Result<UpdateConnection>;

  pub async fn refresh_user(
    &self,
    connection_id: Uuid,
    user: User,
  ) -> anyhow::Result<()>;

  pub async fn unregister(
    &self,
    connection_id: Uuid,
  ) -> anyhow::Result<()>;

  pub async fn publish(
    &self,
    update: UpdateListItem,
  ) -> anyhow::Result<()>;

  pub fn metrics(&self) -> UpdateFanoutMetrics;
}

pub struct UpdateConnection {
  pub id: Uuid,
  pub receiver: mpsc::Receiver<UpdateListItem>,
  // Drop only cancels; the WS owner must await hub.unregister(id).
  registration: UpdateRegistration,
}
```

The channel carries
`HubCommand::{Register, RefreshUser, Unregister, Publish}`. Register,
RefreshUser, Unregister, and Publish include a oneshot acknowledgement, so a completed
registration/refresh is ordered relative to publications and
`send_update().await` means the event has been processed by the hub. A
RefreshUser updates the canonical group record in place and retains the same
connection queue; it cannot create an unsequenced delivery hole. Start exactly
one dispatcher task from the
`OnceLock` initializer. The dispatcher is the sole owner of:

```rust
HashMap<String, UserSubscribers>
// UserSubscribers { user: User, connections: HashMap<Uuid, ConnectionSink> }
// ConnectionSink { sender: mpsc::Sender<UpdateListItem>,
//                  cancel: CancellationToken }
```

`UpdateRegistration::drop` cancels its token but never performs a best-effort
send into the bounded command queue. The dispatcher prunes cancelled sinks on
every command and a one-second maintenance tick. The WebSocket handler wraps
its send/receive loop so every exit path—queue closure, serialization/send
failure, invalid user, sequence overflow, or peer close—then awaits
`unregister(connection_id)` and its acknowledgement. Combine loop and
unregister errors without skipping cleanup. Tests fill the hub command queue,
drop one handle, prove the maintenance sweep removes it, and separately prove
explicit unregister waits for capacity/ack and leaves active connection/user
metrics at zero.

It processes Publish commands one at a time, preserving global Update order.
For one event it snapshots the distinct user groups, calls
`permission_snapshot_provider().can_read_target(&user, &update.target)`
once per user with at most 16 checks in flight, collects all decisions, and
then handles them in stable user-id order. Only after every decision for event
N is applied may it dequeue event N+1. The provider's own double-read guard is
the authorization linearization point.

There is no generation read, per-event `OnceCell`, per-connection permission
cache, or retry loop in this plan. Each new event obtains a fresh provider
decision. An authorization error cancels and removes that user's connections.
A deny keeps the connections but enqueues nothing. An allow uses
`try_send(update.clone())` for every connection; a full or closed queue
cancels and removes that connection. No path silently drops a visible event
while leaving its connection healthy. Remove a `UserSubscribers` group as
soon as its final connection leaves so disconnected user IDs cannot accumulate.

Add hub metrics with a serializable snapshot:

```rust
#[derive(Clone, Debug, serde::Serialize)]
pub struct UpdateFanoutMetrics {
  pub published_events: u64,
  pub authorization_calls: u64,
  pub active_users: usize,
  pub active_connections: usize,
  pub denied_user_events: u64,
  pub authorization_errors: u64,
  pub queue_full_disconnects: u64,
}
```

The injected test authorizer increments `authorization_calls` at the same
call boundary as the production provider.

- [ ] **Step 3: Publish every progress and terminal event through the hub**

In `helpers/update.rs`, import only
`super::channel::update_fanout_hub` and replace the old sender-lock helper
with:

```rust
pub(super) async fn send_update(
  update: UpdateListItem,
) -> anyhow::Result<()> {
  update_fanout_hub().publish(update).await
}
```

Do not import or construct an `UpdateEvent`; that type no longer exists.
`add_update`, `update_update`, `broadcast_update`, and the progress writer
all await this same helper. Publish never coalesces. Add a regression that
publishes InProgress then Complete with the same Update ID through a real test
hub and observes both statuses in order at the registered queue.

- [ ] **Step 4: Make each WebSocket consume its already-authorized queue**

In `api/ws/update.rs`, perform login first, then call
`update_fanout_hub().register(user.clone(), cancel.clone()).await`. The
sender task owns the returned receiver and one
`UpdateStreamSequencer::new(Uuid::new_v4().to_string())`. It stamps only
items dequeued from this authorized connection queue and serializes them to the
socket.

Use a 30-second validity interval. On each tick call
`check_user_valid(&user.id)`; invalid users receive the existing
`INVALID_USER` payload and close. Pass every valid refreshed record to
`refresh_user(connection.id, refreshed_user).await`; the acknowledged
in-place hub command preserves the queue and event order while making an
admin-role change visible to the next authorization decision. Queue closure,
hub cancellation, refresh failure, serialization failure, WS send failure,
and sequence overflow all close the socket and unregister. This close is the
explicit Plan 3 reconnect/full-sync barrier.

Delete `user_can_see_update`, `get_user_permission_on_target`, the Update
broadcast receiver, and per-event `check_user_valid`. Keep the explicit
imports needed by the new loop:
`std::time::Duration`, `uuid::Uuid`, `tracing::warn`,
`tokio::select`, `serde_json::json`, `mogh_error::serialize_error`,
`UpdateStreamSequencer`, and `update_fanout_hub`. Remove now-unused
`anyhow`, `ResourceTarget`, and `PermissionLevel` imports.

- [ ] **Step 5: Prove real provider DB reads do not scale with connections**

Unit tests prove the hub's call cardinality with an injected authorizer:
one and 32 same-user connections both produce one call, while 32 distinct
users produce 32. The real-provider database-read count is a Task 15 staging
gate, because the provider deliberately exposes no metrics API.

Run the one- and 32-connection scenarios in separate processes against the
isolated fixture database. Give each process a unique Mongo `appName`, enable
profiling only on that disposable database, and count timestamp-bounded
read-command entries in `system.profile` for that app name. Warm and seed
before opening the measurement interval. Assert the read counts are positive
and equal, then revoke the real relationship and prove the next event is
denied. The artifact records exact Update IDs, profile interval, matched
profile entry IDs/namespaces/commands, hub authorization calls, and delivery
counts. Restore the prior profiling level in cleanup even when measurement returns an error.

- [ ] **Step 6: Verify imports, ordering, and provider integration**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo test -p komodo_core helpers::channel::tests -- --nocapture
rtk cargo test -p komodo_core helpers::channel::tests::merge_gate_b_stream_contract -- --exact --nocapture
rtk cargo test -p komodo_core api::ws::update -- --nocapture
rtk cargo test -p komodo_client event_compatibility_tests
rtk cargo check -p komodo_core
rtk rg -n 'UpdateEvent|OnceCell|shareable_generation|can_send_with|update_channel' \
  bin/core/src/helpers/channel.rs \
  bin/core/src/helpers/update.rs \
  bin/core/src/api/ws/update.rs
```

Expected: tests/check pass and the final search has no match. Same-user fan-out
performs one provider call per event, delivery order is stable, and a slow
connection is closed rather than silently skipped.

Commit:

```bash
rtk git add bin/core/src/helpers/channel.rs bin/core/src/helpers/update.rs bin/core/src/api/ws/update.rs
rtk git commit -m "perf: centralize ordered update fanout"
```

### Task 15: Run executable staging gates for storage, runtime, fan-out, and RSS

**Files:**
- Modify: `bin/core/src/helpers/update_stream.rs`
- Create: `bin/core/src/helpers/update_stream/staging.rs`
- Modify: `bin/core/src/api/execute/dispatch.rs`
- Modify: `bin/core/src/helpers/procedure_tree.rs`
- Modify: `lib/command/src/budget.rs`
- Modify: `lib/command/tests/bounded_output.rs`
- Create directory: `scripts/performance/`
- Create: `scripts/performance/validate-runtime-backpressure.sh`
- Create directory: `docs/performance/`
- Create: `docs/performance/runtime-backpressure-validation.json`
- Create: `docs/performance/runtime-backpressure-validation.md`

Before implementing fixtures, freeze the measurement protocol in the JSON
schema. Use one dedicated Linux staging class with 4 vCPU and 8 GiB RAM, Core
and Mongo on the same private network, release binaries, and no unrelated
load. Record git SHA, UTC timestamp, `uname -a`, `/etc/os-release`, `lscpu -J`,
`/proc/meminfo:MemTotal`, cgroup CPU/memory limits, CPU governor, Rust/Cargo
versions, Mongo version, and Core/Periphery image or binary revision. A missing
fingerprint field fails the gate.

Freeze this required top-level schema; every string is nonempty and structured
commands remain JSON rather than escaped display text:

```json
{
  "environment": {
    "git_sha": "...",
    "captured_at_utc": "...",
    "uname": "...",
    "os_release": { "ID": "...", "VERSION_ID": "..." },
    "lscpu": { "lscpu": [] },
    "mem_total_bytes": 8589934592,
    "cgroup_version": 2,
    "cgroup_cpu_max": "max 100000",
    "cgroup_memory_max": "8589934592",
    "effective_cpu_count": 4.0,
    "effective_memory_bytes": 8589934592,
    "accepted_class_bounds": {
      "cpu_min": 3.99,
      "cpu_max": 4.01,
      "memory_min_bytes": 8053063680,
      "memory_max_bytes": 9126805504
    },
    "cpu_governor": "performance",
    "rustc_version": "...",
    "cargo_version": "...",
    "mongo_version": "...",
    "core_revision": "...",
    "periphery_revision": "..."
  }
}
```

Use these exact sample rules:

- command RSS: five fresh-process 100 MiB runs; every run must satisfy the
  32 MiB bound and the artifact reports the maximum;
- Procedure progress: five fresh-process repetitions at each 1/10/100 MiB
  size, each with a unique Update ID; every safety bound passes, and reported
  central values are the third value after sorting five samples;
- WebSocket provider: five paired 1-connection/32-connection repetitions with
  fresh app names; read counts must be positive and equal inside every pair;
- deterministic batch matrices: one test process, but every 10/100/1,000 row
  repeats its controlled workload five times and reports maximum peaks;
- API-key concurrency 1/8/32: three independent windows of at least 100
  completed requests and at least five seconds each;
- Periphery stats: three independent intervals, each at least
  `max(5, 2 * stats_interval)` seconds and containing a completed refresh.

For any latency/lag sample array of size `N`, nearest-rank percentile `p` is
`sorted[ceil(p * N) - 1]`; there is no interpolation. Compute p95/p99 inside
each API window, report the median (second sorted value) of the three window
percentiles, and require every individual window to meet safety limits. For
five Procedure/RSS/WS repetitions, report the third sorted value but use the
maximum for upper-bound gates. Retrying or dropping a failed repetition is
forbidden; retain it and fail the batch.

- [ ] **Step 1: Return exact per-Update progress-write evidence**

Instrument the real `ProcedureProgressWriter`; do not estimate Mongo work from
input size. Change `finish` to return:

```rust
#[derive(Clone, Debug, serde::Serialize)]
pub struct ProgressWriterReport {
  pub update_id: String,
  pub source_input_bytes: u64,
  pub accepted_progress_bytes: u64,
  pub mongo_append_payload_bytes: u64,
  pub mongo_update_spec_bson_bytes: u64,
  pub mongo_update_spec_max_bytes: u64,
  pub mongo_update_spec_overhead_max_bytes: u64,
  pub mongo_append_ops: u64,
  pub progress_events: u64,
  pub truncation_marker_appended: bool,
}
```

Add a test-only completion observer to the production scheduler path; do not
refer to an implicit “test result channel”:

```rust
#[cfg(test)]
#[derive(Clone, Debug, serde::Serialize)]
pub struct ProcedureCompletionReport {
  pub update: Update,
  pub writer: ProgressWriterReport,
  pub root_metrics: ProcedureRootMetricsSnapshot,
}

#[cfg(test)]
pub async fn submit_observed(
  &self,
  root: ProcedureRootRequest,
  completed: oneshot::Sender<ProcedureCompletionReport>,
) -> anyhow::Result<Update>;
```

Both `submit` and `submit_observed` call one private `submit_inner`; the only
difference is the optional observer sender stored in root coordinator state.
The finalizer creates the writer report, the coordinator takes its final
per-root metric snapshot at `NodeFinalized`, and it sends the report only after
terminal Mongo persistence and terminal hub publication succeed. A dropped
observer never changes production lifecycle or success. Add a unit test proving
the observed Update ID, writer Update ID, terminal persisted ID, and root
metric ID are identical, plus a dropped-observer test.

Change the writer task handle and `finish` return type to
`JoinHandle<anyhow::Result<ProgressWriterReport>>` and
`anyhow::Result<ProgressWriterReport>`. Because scheduler workers hold cloned
sinks, put only `source_input_bytes: AtomicU64` in a per-writer
`Arc<ProgressCounters>`; use `fetch_update` with `checked_add` in
`push_line` and return an overflow error instead of wrapping. The single
writer task owns all accepted/Mongo/event fields and constructs the final
report.
Update `combine_finish_results` and its dropped-ack/error tests so a task
error still wins and a successful report is returned intact.

Increment `source_input_bytes` from the caller's `line.len()` before adding
the writer's newline delimiter or truncating; this makes it equal the fixture
payload size. Count accepted bytes after delimiter insertion when the buffer
accepts data, and Mongo bytes/ops only after
`append_first_log_stdout` succeeds. Build the Mongo filter and update-pipeline
BSON documents once, use those same values for the driver call, and serialize
`doc! { "q": filter, "u": pipeline }` with `bson::to_vec` before the call.
After success, accumulate that measured update-spec size in
`mongo_update_spec_bson_bytes`, retain the maximum full spec size, and retain
the maximum `spec_bson_bytes - chunk.len()` overhead. Payload bytes alone are
not claimed as Mongo BSON bytes; the measurement excludes only the driver's
transport/session envelope. Increment `progress_events` only after
the real ordered hub acknowledges `send_update`. These counters are owned by
one writer, so they need no process-global reset and are tied to
`update_id`. In Task 12's `FinalizeNode` path, change the writer-success
arm to accept and trace the report; writer failure semantics remain unchanged.
The root staging fixture retains its report for the artifact.

Add `#[cfg(test)] mod staging;` to `update_stream.rs`. The ignored staging
tests run one per process because Core configuration and DB client are
process-wide `OnceLock` values.

- [ ] **Step 2: Build a real Mongo procedure fixture for 1, 10, and 100 MiB**

Create `helpers/update_stream/staging.rs` with ignored Linux test
`procedure_progress_fixture_from_env`. It requires:

```text
KOMODO_DATABASE_URI
KOMODO_DATABASE_DB_NAME
RUNTIME_FIXTURE_BYTES
RUNTIME_FIXTURE_ARTIFACT
```

Refuse to run unless the database name starts with
`komodo_runtime_backpressure_`; never point the fixture at an operator
database. Check the **resolved**
`core_config().database.db_name` after all config-file/env overrides, not
only the raw environment string, before calling `init_db_client()`. Create
one RunProcedure root request with a service-user operator. Acquire a local
`ProcedureRootPermit` guard exactly as the dispatcher does, keep it in scope,
and submit through the production `ProcedureTreeScheduler`. Reuse the scheduler's
existing injected worker seam from Task 9 only to make the single fixture leaf
emit a chosen amount of progress; do not manually drive the writer or manually
persist terminal state. The real coordinator creates the InProgress Update,
writer/sink, finalizer, Mongo appends, ordered progress events, terminal write,
and terminal event. Call the explicit `submit_observed` API and capture its
`ProcedureCompletionReport`; no other metrics/report channel exists.

Before submission, register one fixture admin connection in the ordered hub
and drain it concurrently into an in-memory vector; do not let the 100-item
connection queue fill while a capped run emits roughly 128 flush events. After
root completion, unregister and assert the first event for the exact Update ID
is InProgress, the last is Complete, and all intervening events preserve
order. Wait with a bounded deadline until the drain has observed
`writer.progress_events + 2` matching events before unregistering. Use this
observed vector—not an assumed lifecycle—to populate
`initial_events`, `terminal_events`, and `hub_events`.

Use deterministic 16 KiB UTF-8 lines until exactly
`RUNTIME_FIXTURE_BYTES` source bytes have been submitted. Start a dedicated
`std::thread` sampler before the first line, synchronize it with a
`std::sync::Barrier`, sample `/proc/self/status:VmRSS` every 10 ms, and
retain baseline/peak bytes; a Tokio task is not accepted because scheduler lag
could hide the peak. Stop the sampler only after the scheduler returns the
root `NodeFinalized` acknowledgement.

Query Mongo by that **exact ObjectId**, not by “latest Procedure”. Serialize
the stored BSON to obtain document bytes and count the truncation marker across
all four persisted Log strings. Capture ordered-hub metrics before/after to
prove terminal publication. Atomically write
`RUNTIME_FIXTURE_ARTIFACT.tmp` then rename it to the requested path with:

```json
{
  "requested_input_bytes": 10485760,
  "update_id": "object-id",
  "writer": {
    "update_id": "object-id",
    "source_input_bytes": 10485760,
    "accepted_progress_bytes": 8323000,
    "mongo_append_payload_bytes": 8323000,
    "mongo_update_spec_bson_bytes": 8400000,
    "mongo_update_spec_max_bytes": 66000,
    "mongo_update_spec_overhead_max_bytes": 500,
    "mongo_append_ops": 128,
    "progress_events": 33,
    "truncation_marker_appended": true
  },
  "mongo_document_bytes": 8330000,
  "marker_count": 1,
  "terminal_status": "Complete",
  "terminal_success": true,
  "procedure_runtime": {
    "root_update_id": "object-id",
    "peak_ready": 1,
    "nodes": 1,
    "peak_expand": 1,
    "peak_leaf": 1
  },
  "initial_events": 1,
  "terminal_events": 1,
  "hub_events": 35,
  "rss_baseline_bytes": 0,
  "rss_peak_bytes": 0,
  "rss_peak_delta_bytes": 0
}
```

The numbers above illustrate the schema; the test writes measured values. Its
assertions are:

- requested and writer source bytes are identical;
- writer `update_id` and queried Mongo `_id` are identical;
- terminal `procedure_root_metrics.root_update_id` equals both IDs and its
  ready/node/worker peaks remain inside the fixed limits;
- stored status is Complete and the terminal Log is present;
- 1 MiB has zero markers; 10 and 100 MiB have exactly one;
- `mongo_document_bytes < 9 * 1024 * 1024`;
- exactly one initial and one terminal event are observed, and
  `hub_events == writer.progress_events + 2`;
- `rss_peak_delta_bytes <= 32 * 1024 * 1024`.

Delete only the exact fixture Update at test teardown after the artifact has
been durably renamed. Preserve it on assertion failure and print the ID so
operators can inspect it.

- [ ] **Step 3: Emit machine-readable 10/100/1,000 batch evidence**

Name the table-driven tests from Tasks 9 and 10 exactly:

```text
api::execute::dispatch::tests::dispatch_batch_matrix_10_100_1000
helpers::procedure_tree::tests::procedure_batch_matrix_10_100_1000
budget::tests::command_batch_matrix_10_100_1000
```

Each test asserts its limits internally and, when
`RUNTIME_BATCH_ARTIFACT_DIR` is set, atomically writes one class-named JSON
artifact containing a three-row `scenarios` array. Every row has input count
and a `repetitions` array of exactly five fresh controlled workload runs. Each
repetition records its ordinal 1–5, input count, ordered result count, rejected
count, and all relevant active/peak/queue metrics; the row also records a
`maximum_peaks` summary computed from those five raw records. Required bounds
are leaf 32, per-key 4, orchestrator 6,
Procedure root 8, ready 256, nodes 4,096, expansion 8, tree leaf 32, user
commands 6, and monitor commands 2. The 1,000-item cases must exercise the
dispatcher/tree/command code, not merely acquire semaphores in a loop. Use
controlled short work so all five repetitions in every matrix scenario expect
ordered result count equal to input and rejected count zero; reset test metrics
between repetitions and never retry/drop a failed one. Queue-overflow behavior remains in its
separate deterministic tests. A dispatcher batch test must await the terminal
test-task acknowledgement for every returned Update ID before taking its final
metrics snapshot; a detached initial response is not completion evidence.

- [ ] **Step 4: Add the real-provider WebSocket DB-read fixture**

In the same staging module add ignored test
`websocket_provider_reads_for_connection_count_from_env`. In addition to the
isolated DB variables it requires `RUNTIME_WS_CONNECTIONS` (only 1 or 32),
`RUNTIME_WS_ARTIFACT`, unique `KOMODO_DATABASE_APP_NAME` for the real provider,
unique `KOMODO_DATABASE_CONTROL_APP_NAME` for profiler control, and the
explicit safety flag `RUNTIME_ALLOW_MONGO_PROFILER=1`.
It repeats the resolved-config database-name safety assertion before enabling
profiling.

Define a local `seed_websocket_permission_fixture` in this staging module; Plan
1 exports no fixture helper. It creates exact isolated User, Server, Permission,
Update, and clean `PermissionCacheState/global` documents before provider
initialization, returns their IDs plus a cleanup guard, and refuses any database
whose resolved name lacks the required prefix. Use the real guarded
`UpdatePermissionOnTarget` resolver for the later revoke; direct collection
writes are allowed only during pre-provider fixture seeding. Seed one
admin-free user, target, and read relationship. Create a second Mongo client
from the same URI/database with only the control app name. All profiling-level
commands, server-time bounds, and `system.profile` queries use this control
client; initialize and warm the real `PermissionSnapshotProvider` only through
the normal configured provider client. Enable profiling level 2 only on the
disposable database. Read Mongo server time for the start bound through the
control client, register the requested number of
same-user connections, publish a unique Update through the real hub, await all
deliveries, then read server time for the end bound.

After the end bound, query `system.profile` through the control client for this
database, the unique provider `appName`, and `ts >= start && ts <= end`.
Assert no matched entry has the control app name. Count only read operations:
`op == "query"` or command documents containing
`find`/`aggregate`/`count`/`distinct`/`getMore` (including
`op == "getmore"`). The later profile query cannot
count itself because it is outside the end timestamp. Record every matched
entry's timestamp, namespace, operation, and command name, not just a scalar.
Require a positive read count, exactly one hub authorization call, and
delivery count equal to `RUNTIME_WS_CONNECTIONS`. All hub assertions use
before/after metric deltas, not process-lifetime absolute counters.

Then revoke the relationship, open a second bounded profile interval, publish
a second unique Update, and assert zero deliveries plus one fresh
authorization call. Capture the prior database profiling level. Structure the
test as `measure().await` followed by an unconditional async cleanup result
so profiling is restored to that exact prior level and only
the fixture records are deleted on both success and returned-error paths; do
not put panicking assertions before cleanup. Atomically write the connection
count, user/target, both Update IDs, both app names, interval bounds, matched profile entries,
read count, hub calls, and deliveries to `RUNTIME_WS_ARTIFACT`. Plan 2 does
not call any non-existent provider metrics API.

- [ ] **Step 5: Implement the validation script around timestamped windows**

Run `rtk mkdir -p scripts/performance docs/performance` because neither
directory exists on the audited base.

Create `scripts/performance/validate-runtime-backpressure.sh` with
`set -euo pipefail`. The committed script uses normal `cargo`, `curl`,
`jq`, required `mongosh`, and coreutils;
it must not invoke local `rtk`. Require:

```text
KOMODO_ADDRESS
KOMODO_API_KEY
KOMODO_API_SECRET
KOMODO_DATABASE_URI
KOMODO_DATABASE_DB_NAME
CORE_JSON_LOG
PERIPHERY_JSON_LOG
PERIPHERY_STATS_INTERVAL_SECONDS
CORE_REVISION
PERIPHERY_REVISION
```

Reject a non-isolated DB name. Create a temporary artifact directory and keep
it on failure. Define `run_id="$(date +%s)-$$"` once and use it in every
profile app name/artifact. Never print or persist API secrets or the Mongo URI;
artifacts contain only isolated IDs and measurements. Execute these fixture
processes serially. Fail immediately unless `uname -s` is `Linux`; a
zero-test result on another OS is not RSS evidence. Preflight
`command -v cargo curl jq mongosh lscpu uname rustc git nproc` and fail with the
missing executable name. Before workloads, capture `git rev-parse HEAD`, UTC
RFC3339 time, `uname -a`, parsed `/etc/os-release`, `lscpu -J`, MemTotal
converted from KiB to bytes, cgroup-v2 `cpu.max`/`memory.max`, every distinct CPU
governor, `rustc --version`, `cargo --version`, and the two required revisions.
Query `db.runCommand({ buildInfo: 1 }).version` through `mongosh` against the
already-validated isolated database and store only the returned version, never
the URI. Assemble and `jq -e` the exact `environment` object before running the
first repetition; any null/empty field exits nonzero.

The staging class is normative, not merely recorded. Require readable
`/sys/fs/cgroup/cgroup.controllers`, `cpu.max`, and `memory.max`; otherwise fail
with `cgroup v2 is required for the staging gate` rather than falling back to
cgroup v1 or unconstrained host values. Compute effective CPUs as
`min(nproc, quota / period)` when cgroup-v2 `cpu.max` is numeric, otherwise
`nproc`; require `3.99 <= effective_cpus <= 4.01`. Compute effective memory as
the smaller of `/proc/meminfo:MemTotal` and numeric `memory.max` (or MemTotal
when unlimited); require it between 7.5 GiB and 8.5 GiB to allow kernel/cgroup
accounting around the nominal 8 GiB class. Store both computed values and the
accepted bounds in `environment`; fail before workloads when either class gate
does not pass.

```bash
for repetition in 1 2 3 4 5; do
  COMMAND_RSS_ARTIFACT="$artifact_dir/command-rss-$repetition.json" \
    cargo test -p command --release --test bounded_output \
      one_hundred_mib_producer_adds_at_most_thirty_two_mib_rss \
      -- --ignored --exact --nocapture --test-threads=1
done

for bytes in 1048576 10485760 104857600; do
  for repetition in 1 2 3 4 5; do
    RUNTIME_FIXTURE_BYTES="$bytes" \
    RUNTIME_FIXTURE_ARTIFACT="$artifact_dir/procedure-$bytes-$repetition.json" \
      cargo test -p komodo_core --release \
        helpers::update_stream::staging::procedure_progress_fixture_from_env \
        -- --ignored --exact --nocapture --test-threads=1
  done
done

for repetition in 1 2 3 4 5; do
  for connections in 1 32; do
    KOMODO_DATABASE_APP_NAME="runtime-ws-$run_id-$repetition-$connections" \
    KOMODO_DATABASE_CONTROL_APP_NAME="runtime-ws-control-$run_id-$repetition-$connections" \
    RUNTIME_ALLOW_MONGO_PROFILER=1 \
    RUNTIME_WS_CONNECTIONS="$connections" \
    RUNTIME_WS_ARTIFACT="$artifact_dir/ws-provider-$repetition-$connections.json" \
      cargo test -p komodo_core --release \
        helpers::update_stream::staging::websocket_provider_reads_for_connection_count_from_env \
        -- --ignored --exact --nocapture --test-threads=1
  done
done

RUNTIME_BATCH_ARTIFACT_DIR="$artifact_dir/batches" \
  cargo test -p komodo_core --release \
    api::execute::dispatch::tests::dispatch_batch_matrix_10_100_1000 \
    -- --exact --nocapture
RUNTIME_BATCH_ARTIFACT_DIR="$artifact_dir/batches" \
  cargo test -p komodo_core --release \
    helpers::procedure_tree::tests::procedure_batch_matrix_10_100_1000 \
    -- --exact --nocapture
RUNTIME_BATCH_ARTIFACT_DIR="$artifact_dir/batches" \
  cargo test -p command --release \
    budget::tests::command_batch_matrix_10_100_1000 \
    -- --exact --nocapture
```

Validate all fifteen Procedure artifacts and five command-RSS artifacts with
`jq -e`; group Procedure results by input size and apply the frozen
median/maximum rules:

- the exact command-RSS filename/ordinal set is 1–5; every object contains
  numeric `rss_baseline_bytes`, `rss_peak_bytes`, `rss_peak_delta_bytes`, and
  `retained_output_bytes` plus boolean `truncation_marker_present`; peak is not
  below baseline, delta equals their saturated difference, every delta is at
  most 32 MiB, retained bytes stay inside the frozen output cap, and every
  marker is true;
- all Update IDs are non-empty and unique;
- each requested/source byte count equals its 1/10/100 MiB scenario;
- the 1 MiB marker count is zero and 10/100 MiB marker counts are one;
- all statuses are Complete, successes true, document bytes below 9 MiB, and
  RSS deltas at most 32 MiB;
- each event count is `progress_events + 2`, with exactly one initial and one
  terminal event;
- 10 and 100 MiB `accepted_progress_bytes` are exactly equal, and each
  scenario's `mongo_append_payload_bytes == accepted_progress_bytes`;
- 10/100 MiB `mongo_update_spec_bson_bytes` differ by no more than one
  64 KiB batch plus one measured
  `mongo_update_spec_overhead_max_bytes`. Timer-driven flush boundaries may
  change the append-op count, so exact command-byte/op equality is not a gate.
  The 1 MiB case must remain smaller and linear below the cap.

Validate all five paired `ws-provider-{repetition}-{1,32}.json` sets with
`jq -e`: profiler read counts are positive and equal inside each pair, every
counted profile entry lies inside its
recorded server-time interval and has the expected unique provider app name,
no counted entry has the paired control app name,
authorization calls are one per event, deliveries are 1/32 before revocation
and zero after it.

Validate all batch artifacts with `jq -e`: the exact input set is
`[10,100,1000]` for dispatcher, tree, and command classes; every row has
exactly five repetitions with ordinals `[1,2,3,4,5]` and each repetition's
input equals its row input; ordered results equal input, rejection count is
zero, and every raw and maximum-summary class/per-key/per-root peak is at or
below its declared constant. Missing rows/repetitions fail the script.

- [ ] **Step 6: Measure Core lag/busy inside each API workload window**

Use one cookie-free authenticated request for every preflight and worker; this
is the bcrypt path under test:

```bash
curl --silent --show-error --request POST \
  --header 'content-type: application/json' \
  --header 'cookie:' \
  --header "x-api-key: $KOMODO_API_KEY" \
  --header "x-api-secret: $KOMODO_API_SECRET" \
  --data '{}' \
  "$KOMODO_ADDRESS/read/GetVersion"
```

Before opening a window, require that exact request to return 200 with a
nonempty `.version`; repeat with `${KOMODO_API_SECRET}__invalid` and require
401. Do not use JWT/cookies, a public route, static asset, health endpoint, or
another Read operation. Worker commands add only curl timing/status output to
this exact request and discard the version body after validating JSON; neither
secret is written to command lines in artifacts or logs.

For API-key concurrency 1, 8, and 32, run three independent worker windows;
each lasts at least five seconds and continues until at least 100 requests have
completed.
Write one JSON line per request containing wall-clock start/end Unix
milliseconds, status, and `curl time_total`. Record a scenario
`window_start_unix_ms` immediately before workers and
`window_end_unix_ms` after they join, then wait two seconds for tracing
flush.

For each scenario select only Core log records where
`fields.event == "runtime_budget_metrics"` and the observer's complete
window is wholly inside the scenario interval. Fail if there is no such
window. For every selected one assert:

```text
tokio_lag_p99_ms < 10
0 <= tokio_busy_pct <= 100 and it is not null
queued_execution_total/peak_queued_execution_total <= 256
queued_leaf/peak_queued_leaf <= 256
every queued_by_key/peak_queued_by_key value <= 16
active_leaf/peak_leaf <= 32
every active_by_key/peak_by_key value <= 4
queued_orchestrator/peak_queued_orchestrator <= 32
active_orchestrator/peak_orchestrator <= 6
queued_procedure_root/peak_queued_procedure_root <= 32
active_procedure_root/peak_procedure_root <= 8
active_procedure_expand/peak_procedure_expand <= 8
active_procedure_tree_leaf/peak_procedure_tree_leaf <= 32
every procedure_by_root value has ready/peak_ready <= 256, nodes <= 4096,
  active_expand/peak_expand <= 8, and active_leaf/peak_leaf <= 32
active_monitor/peak_monitor <= 16
every active_monitor_by_key/peak_monitor_by_key value <= 2
```

Require all curl statuses to be 2xx and calculate nearest-rank p95/p99 from
each window's own latency lines, then the median of the three window
percentiles as frozen above. Never select all historical records from
`CORE_JSON_LOG`; timestamps tie evidence to the load that produced it.

- [ ] **Step 7: Measure Periphery scheduler and stats-refresh lag**

Record three independent Periphery intervals and wait for
`max(5, 2 * PERIPHERY_STATS_INTERVAL_SECONDS)` seconds in each. Select only
`fields.event == "periphery_runtime_metrics"` windows wholly inside that
interval. Require at least one selected window with
`stats_refresh_count > 0`; fail if all selected windows missed a refresh.
For every window assert non-null `tokio_busy_pct`,
`tokio_lag_p99_ms < 10`, user command active/peak at most six, monitor
active/peak at most two, ActionHost active/peak at most six (normally zero on
Periphery), queued/peak-queued user commands at most 64, and no
negative stats duration. A window with count zero must omit max/last; a window
with positive count must contain non-negative max and a last-completed
timestamp. Record
`stats_refresh_max_ms` and last-completed timestamp in the final artifact.

- [ ] **Step 8: Assemble evidence and compatibility gates**

The script atomically writes
`docs/performance/runtime-backpressure-validation.json` with:

- the complete required `environment` object captured before workloads;
- all five raw command-RSS artifacts plus median and maximum summaries;
- Procedure objects including exact Update IDs, source/Mongo/document/event/RSS
  measurements;
- Core API latency plus every selected one-second lag/busy/budget window;
- Periphery stats-refresh/lag/busy/command windows;
- dispatcher, Procedure-tree, and command batch matrices for 10/100/1,000;
- real provider/hub Mongo-read evidence for WebSockets.

It also renders the JSON into
`docs/performance/runtime-backpressure-validation.md` with columns
`scenario`, `input/concurrency`, `update_id`, `p95_ms`, `p99_ms`,
`tokio_lag_p99_ms`, `tokio_busy_pct`, `peak_active`,
`mongo_append_payload_bytes`, `mongo_update_spec_bson_bytes`,
`mongo_document_bytes`, `events`, and
`rss_peak_delta_bytes`. Missing values display as `n/a`; missing required
measurements fail before rendering.

Run syntax and focused gates:

```bash
rtk chmod +x scripts/performance/validate-runtime-backpressure.sh
rtk bash -n scripts/performance/validate-runtime-backpressure.sh
rtk cargo test -p komodo_core helpers::channel::tests -- --nocapture
rtk cargo test -p komodo_core helpers::update_stream::tests -- --nocapture
rtk cargo test -p komodo_core api::execute::dispatch::tests -- --nocapture
rtk cargo test -p komodo_core helpers::procedure_tree::tests -- --nocapture
rtk cargo test -p command budget::tests -- --nocapture
rtk scripts/performance/validate-runtime-backpressure.sh
```

For rolling compatibility, also run
`rtk cargo test -p komodo_client event_compatibility_tests`. Record manual
old-Core/new-UI and new-Core/old-UI smoke checks. Before Merge Gate B, add and
run `helpers::channel::tests::merge_gate_b_stream_contract`, covering Core
restart, same-Core reconnect, simulated cross-Core reconnect, hidden events,
broadcast lag, full connection queues, sequence overflow, and old/new envelope
deserialization. Every authenticated connection gets a new epoch and starts at
sequence one; denied events consume no sequence; lag/full/overflow closes the
connection rather than producing a gap; reconnect never reuses an epoch.

Merge Gate B is the additive backend producer/compatibility contract only. It
does not claim that the not-yet-merged Plan 3 UI has handled an injected gap,
duplicate, or out-of-order frame. Plan 3 checkpoint 3 defines Merge Gate C:
its deterministic consumer and socket-integration tests must cover those
inputs plus old-Core fallback before any 60-second polling change may merge.
Record a new/new forced queue-full reconnect/full-sync smoke at Gate C, not as
a circular prerequisite for Gate B.

- [ ] **Step 9: Run repository closure and open checkpoint 4**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo test --workspace
rtk cargo build --workspace
rtk yarn --cwd client/core/ts build
rtk rg -n 'procedure_admission' \
  bin/core/src/api/execute/procedure.rs \
  bin/core/src/helpers/procedure.rs \
  bin/core/src/helpers/procedure_tree.rs
rtk rg -n 'shareable_generation|can_send_with|OnceCell' \
  bin/core/src/helpers/channel.rs \
  bin/core/src/helpers/update.rs \
  bin/core/src/api/ws/update.rs
```

Expected: format, workspace tests/build, and TypeScript build pass; only the
explicit isolated-DB/RSS fixtures are ignored normally; the final search has
no match.

Commit and open the fork-only PR:

```bash
rtk git add \
  bin/core/src/helpers/update_stream.rs \
  bin/core/src/helpers/update_stream/staging.rs \
  bin/core/src/api/execute/dispatch.rs \
  bin/core/src/helpers/procedure_tree.rs \
  lib/command/src/budget.rs \
  scripts/performance/validate-runtime-backpressure.sh \
  docs/performance/runtime-backpressure-validation.json \
  docs/performance/runtime-backpressure-validation.md
rtk git commit -m "test: validate runtime backpressure budgets"
rtk git push -u origin runtime-update-events
rtk gh pr create --repo intezya/komodo --base main --head runtime-update-events --title "Bound update progress and websocket fanout" --body "Caps persisted Update logs at 8 MiB, appends Procedure progress in bounded delta batches, routes execution through fair observable budgets, centralizes ordered per-user Update authorization, and includes timestamp-bound Core/Periphery/Mongo/RSS validation evidence. Verification: cargo test --workspace; cargo build --workspace; yarn --cwd client/core/ts build; scripts/performance/validate-runtime-backpressure.sh."
```

Expected: the PR targets `intezya/komodo`, Plan 1 is in its base, every
staging gate has a committed evidence row, and rollback needs no storage
migration because logs remain in the existing `Update.logs` fields.

## Final rollout and rollback order

1. Merge and deploy checkpoint 1. Compare Tokio lag during bcrypt concurrency
   1/8/32 and Periphery stats refresh before proceeding.
2. Merge checkpoint 2. Exercise 1/10/100 MiB output and log-tail extremes;
   rollback is a binary rollback because there is no storage change.
3. Merge checkpoint 3. Observe queue-full, admission-timeout, monitor-skip,
   per-key leaf, Action, per-root Procedure-tree, and Periphery command
   counters. Increase no limit until a representative trace proves sustained
   safe headroom.
4. Merge Plan 1 checkpoint 4, then rebase, merge, and deploy this plan's
   checkpoint 4. New event fields are optional and bounded logs remain in the
   old field, so old Core/UI combinations stay parse-compatible.
5. If checkpoint 4 must be rolled back, deploy the previous Core. Already
   truncated Updates retain the explicit marker; no dual read, backfill, or
   collection migration is needed. Keep Plan 1's permission-cache kill switch
   active according to its own rollback procedure whenever an old Core binary
   is present.

## Execution handoff

Plan complete and saved to
`docs/superpowers/plans/2026-07-10-komodo-runtime-backpressure-events.md`.
Execute with `superpowers:subagent-driven-development` for a fresh worker and
two-stage review per task, or `superpowers:executing-plans` for checkpoint
batches in one session. Stop before checkpoint 4 until Merge Gate A is merged.
