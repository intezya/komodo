# Komodo Core Data Path Performance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Core permission reads, monitoring inventory loads, state refreshes, and resource-cache updates use bounded MongoDB work that does not grow with every server or resource mutation.

**Architecture:** Ship four independently deployable checkpoints. First establish query budgets and repository-managed indexes, then batch monitoring and latest-state reads, replace the monolithic resource cache with atomic per-type snapshots and dirty read-through repair, and finally add a fail-closed Mongo generation protocol plus generation-keyed permission snapshots. MongoDB remains authoritative; every cache has an explicit bypass and repair path.

**Tech Stack:** Rust 2024, Tokio, MongoDB Rust driver 3.6 through `mungos`, `arc-swap`, Axum/`mogh_resolver` APIs, Bash, `mongosh`, Cargo tests.

---

## Scope, facts, and fixed budgets

This plan owns exactly these approved P1 findings:

1. Non-admin `ListUpdates` and `ListAlerts` permission fan-out.
2. Per-server/per-swarm monitoring relationship queries and their missing repository-managed indexes.
3. Build, Repo, Procedure, and Action latest-state N+1 refreshes.
4. Eleven sequential `AllResources` collection reads every fifteen seconds and after generic resource mutations.

Confirmed at `main@1334baee`:

- `bin/core/src/permission.rs:364-559` computes eleven resource-type clauses sequentially.
- `bin/core/src/monitor/mod.rs:265-318` executes three relationship queries per server and two per swarm.
- `lib/database/src/lib.rs:245-255` creates only `name` and `tags` resource indexes.
- `bin/core/src/resource/{build,repo,procedure,action}.rs` runs one or two latest-update reads per resource every sixty seconds.
- `bin/core/src/helpers/all_resources.rs:24-76` loads eleven collections sequentially, while `bin/core/src/resource/mod.rs:547,644,729,809,876` waits for that reload after generic mutations.
- There is no `ResourceGroup` entity or collection at this revision. The approved spec's “resource-group membership” category maps to a `Permission` row whose `user_target` is `UserGroup` and whose `resource_target` identifies one resource. Do not invent a second group model.

Production index state is an assumption until Task 1 captures a read-only
`getIndexes()` inventory from production. Run every `explain("executionStats")`
and all load work only against its staging clone; do not profile or load-test
production.

Freeze these exit budgets before any behavior-changing checkpoint:

| Path | Workload | Budget |
|---|---|---|
| Warm non-admin `ListUpdates`/`ListAlerts` | 30 measured requests after 5 warmups, 1/100/1,000 resources | no more than 4 Mongo commands after authentication; p95 no more than 250 ms on the declared staging class |
| Cold permission snapshot | first request after local snapshot clear, 1/100/1,000 resources | no more than 7 Mongo commands after authentication; command count is identical at all three sizes (two endpoint reads, two guard reads, current User, groups, permissions); no more than 20,000 documents examined or 8 MiB of profiler-recorded response bytes per endpoint sample |
| Server monitoring inventory | one complete background cycle | exactly 4 logical `find` starts; cursor `getMore` <= 12; profiler response bytes <= 64 MiB |
| Swarm monitoring inventory | one complete background cycle | exactly 3 logical `find` starts; cursor `getMore` <= 8; profiler response bytes <= 64 MiB |
| State refresh | Build, Repo, Procedure, or Action cycle | exactly 1 logical `aggregate` start per type; cursor `getMore` <= 4; response bytes <= 16 MiB; indexed lookup examines at most 3 Update rows per resource |
| Relationship indexes | representative 1,000-resource staging fixture | `IXSCAN` and `totalDocsExamined / max(nReturned, 1) <= 5` |
| Resource mutation cache work | one generic or specialized resource write | no full eleven-type reload; publish or dirty every actually affected resource type |
| Dirty resource cache | injected post-commit refresh failure | authoritative read-through immediately; clean snapshot within 5 seconds |

Use one staging class for every before/after sample: 4 vCPU, 8 GiB RAM, Core and MongoDB on the same private network, release binaries, and no other load generator. Record 30 samples, report p50/p95, and retain raw JSON as CI artifacts. Do not claim a percentage speedup from a single run.

## File map

**Checkpoint 1 — measurement and indexes**

- Create: `docs/performance/core-data-path-budgets.md` — immutable workload, query, latency, and explain-plan gates.
- Create: `scripts/performance/inspect-core-data-indexes.js` — safe `mongosh` index inventory and representative explains.
- Create: `scripts/performance/inventory-core-data-indexes.js` — production-safe read-only `getIndexes()` inventory with no explains.
- Create: `scripts/performance/profile-core-data-paths.sh` — isolated-staging
  HTTP request timings only.
- Create: `bin/core/src/api/read/staging.rs` and
  `scripts/performance/profile-core-resolver-commands.sh` — ignored direct-resolver
  command windows in processes with no Core background loops.
- Create: `scripts/performance/validate-core-data-fixture.js` — manifest-backed user/group/grant/denial and collection-count assertions.
- Create: `lib/database/src/indexes.rs` — idempotent key-pattern index reconciliation.
- Modify: `lib/database/src/lib.rs:29-113,245-255` — register relationship and latest-update indexes.

**Checkpoint 2 — batched monitoring and state reads**

- Create: `bin/core/src/monitor/inventory.rs` — collection-sized loads grouped by server/swarm.
- Modify: `bin/core/src/monitor/mod.rs:38-80,103-128,259-325` — consume one server inventory per loop.
- Modify: `bin/core/src/monitor/swarm.rs:46-60,77-99` — consume one swarm inventory per loop.
- Create: `bin/core/src/helpers/latest_states.rs` — one aggregation and pure state reducer per resource type.
- Create: `scripts/performance/explain-latest-state-aggregations.js` — executable full-pipeline IXSCAN and examined-row gate.
- Create: `scripts/performance/profile-core-background-cycles.sh` and `capture-core-background-profile.js` — comment-scoped logical/cursor work gates.
- Modify: `bin/core/src/helpers/mod.rs` — register `latest_states`.
- Modify: `bin/core/src/resource/build.rs:229-253,316-380` — batch Build state refresh.
- Modify: `bin/core/src/resource/repo.rs:207-232,293-325` — batch Repo state refresh.
- Modify: `bin/core/src/resource/procedure.rs:369-426` — batch Procedure state refresh.
- Modify: `bin/core/src/resource/action.rs:163-220` — batch Action state refresh.

**Checkpoint 3 — atomic per-type resource snapshots**

- Rewrite: `bin/core/src/helpers/all_resources.rs:1-78` — per-type `ArcSwap` snapshots, dirty generations, read-through, and repair.
- Modify: `bin/core/src/state.rs:1-4,23-29,211-215` — store `AllResourcesCache` rather than one monolithic `ArcSwap`.
- Modify: `bin/core/src/resource/refresh.rs:14-40` — one-second dirty repair and parallel fifteen-second nonblocking cross-Core convergence refresh.
- Modify generic and specialized resource writers — publish returned rows or dirty every affected type after authoritative writes.
- Modify cache consumers: `bin/core/src/helpers/procedure.rs:672`, `resource/{build,stack,sync}.rs`, `api/{execute,write}/sync.rs`, `api/read/toml.rs`, and `sync/{deploy,execute,resources,toml,user_groups,view}.rs` — inject one dirty-aware snapshot at async boundaries.

**Checkpoint 4 — permission generation and snapshots**

- Create: `lib/database/src/permission_state.rs` — authoritative singleton document.
- Modify: `lib/database/src/lib.rs:4-28,45-113` — typed `PermissionCacheState` collection and disabled-by-default initialization.
- Create: `bin/core/src/permission/mutation.rs` — CAS mutation guard and fail-closed finalization.
- Create: `bin/core/src/permission/snapshot.rs` — generation-keyed snapshot and `PermissionSnapshotProvider`.
- Modify: `bin/core/src/permission.rs:1-38,85-360,364-560` — reuse one group/permission load and targeted query scope.
- Modify: `bin/core/src/api/read/{update,alert}.rs` — consume the provider.
- Modify mutation entry points: `bin/core/src/api/write/{permissions,user_group,user}.rs`, `bin/core/src/helpers/mod.rs:200-234`, `bin/core/src/resource/mod.rs:473-552,821-945`, `bin/core/src/startup.rs:524-582`.
- Create: `scripts/performance/permission-cache-control.js` — runtime disable/enable/recovery control with CAS.
- Modify: `bin/core/src/api/read/staging.rs` and
  `scripts/performance/profile-core-resolver-commands.sh` — add
  generation-forced cold mode to the isolated resolver command harness.
- Create: `scripts/performance/profile-cold-permission-snapshot.sh` — invoke
  that isolated cold mode and enforce command/work budgets.
- Create: `scripts/performance/verify-permission-cache-cross-core.sh` — two-Core revocation and kill-switch test.

## Ordered PR checkpoints

1. Branch `core-data-measure-indexes`: Tasks 1–3. Merge only after staging index inventory and pre/post explains are attached.
2. Branch `core-data-batch-refresh`: Tasks 4–6. Start from checkpoint 1; merge only after fixed query budgets pass.
3. Branch `core-data-resource-cache`: Tasks 7–9. Start from checkpoint 2; merge only after fault-injected read-through converges inside five seconds.
4. Branch `core-data-permission-snapshots`: Tasks 10–14. Start from checkpoint 3; this is Merge Gate A for Plan 2. Snapshot reads stay disabled until every Core runs guarded mutation code.

Every checkpoint must pass `rtk cargo fmt --all -- --check`, `rtk cargo test -p database`, and `rtk cargo test -p komodo_core` before opening a PR. Target only `intezya/komodo`.

### Task 1: Freeze the measurement protocol and capture the pre-change baseline

**Files:**
- Create: `docs/performance/core-data-path-budgets.md`
- Create: `scripts/performance/inspect-core-data-indexes.js`
- Create: `scripts/performance/inventory-core-data-indexes.js`
- Create: `scripts/performance/profile-core-data-paths.sh`
- Create: `scripts/performance/profile-core-resolver-commands.sh`
- Create: `scripts/performance/validate-core-data-fixture.js`
- Create: `bin/core/src/api/read/staging.rs`
- Modify: `bin/core/src/api/read/mod.rs`

- [ ] **Step 1: Create the fixed budget document**

Write `docs/performance/core-data-path-budgets.md` with this complete content:

```markdown
# Core data-path performance budgets

These budgets apply to the Komodo P1 Core data-path program.

## Staging class

- Core: release binary, 4 vCPU, 8 GiB RAM
- MongoDB: same private network, no unrelated load
- Endpoint fixtures: exactly 1, 100, and 1,000 readable workload documents in `Server` plus one fixed denied sentinel, for 2, 101, and 1,001 total Server rows; the same non-admin user, one user group, one direct grant, one inherited grant, and that denied target are retained in every fixture
- Refresh fixtures: exactly 1, 100, and 1,000 documents in the resource type being measured; record all collection counts beside each artifact
- Sampling: 5 warmups followed by 30 recorded requests; report p50 and p95 from the raw sample array

## Gates

- Warm ListUpdates and ListAlerts: <= 4 Mongo commands after authentication and p95 <= 250 ms.
- Cold permission snapshot: <= 7 Mongo commands after authentication; equal command count at all fixture sizes; <= 20,000 documents examined and <= 8 MiB profiler-recorded response bytes per endpoint sample.
- Server monitor cycle: exactly 4 logical finds including the server list;
  <= 12 getMore operations and <= 64 MiB profiler response bytes.
- Swarm monitor cycle: exactly 3 logical finds including the swarm list;
  <= 8 getMore operations and <= 64 MiB profiler response bytes.
- Build, Repo, Procedure, and Action refresh: exactly 1 logical aggregate per
  type; <= 4 getMore operations, <= 16 MiB response bytes, and indexed lookup
  examines <= 3 Update rows per resource.
- Relationship queries: IXSCAN and totalDocsExamined / max(nReturned, 1) <= 5.
- Resource mutation: no eleven-type reload.
- Dirty resource type: authoritative read-through immediately and repair <= 5 seconds.

Raw runs are CI artifacts named core-data-path-<git-sha>-<fixture-size>.json. Never commit credentials or MongoDB URIs.
```

- [ ] **Step 2: Create the index-inspection script**

First create `scripts/performance/inventory-core-data-indexes.js`:

```javascript
for (const name of [
  "Swarm",
  "Server",
  "Stack",
  "Deployment",
  "Build",
  "Repo",
  "Procedure",
  "Action",
  "ResourceSync",
  "Builder",
  "Alerter",
  "Update",
  "Permission",
  "UserGroup",
]) {
  print(EJSON.stringify({
    kind: "indexes",
    collection: name,
    indexes: db.getCollection(name).getIndexes(),
  }));
}
```

This script performs metadata reads only. The separate staging script below
contains the representative explains.

Write `scripts/performance/inspect-core-data-indexes.js`:

```javascript
const fs = require("fs");
const manifestPath = process.env.FIXTURE_MANIFEST;
if (!manifestPath) throw new Error("FIXTURE_MANIFEST is required");
const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
for (const field of [
  "monitor_server_id",
  "monitor_swarm_id",
  "latest_state_build_id",
]) {
  if (!manifest[field]) throw new Error(field + " is required");
}
if (!manifest.latest_state_operation_counts) {
  throw new Error("latest_state_operation_counts is required");
}

for (const name of [
  "Swarm",
  "Server",
  "Stack",
  "Deployment",
  "Build",
  "Repo",
  "Procedure",
  "Action",
  "ResourceSync",
  "Builder",
  "Alerter",
  "Update",
  "Permission",
  "UserGroup",
]) {
  print(EJSON.stringify({
    kind: "indexes",
    collection: name,
    indexes: db.getCollection(name).getIndexes(),
  }));
}

function explain(collection, filter, sort = undefined, limit = undefined) {
  let cursor = db.getCollection(collection).find(filter);
  if (sort) cursor = cursor.sort(sort);
  if (limit) cursor = cursor.limit(limit);
  const result = cursor.explain("executionStats");
  const stats = result.executionStats;
  print(EJSON.stringify({
    kind: "explain",
    collection,
    filter,
    sort,
    winningPlan: result.queryPlanner.winningPlan,
    nReturned: stats.nReturned,
    totalDocsExamined: stats.totalDocsExamined,
    ratio: stats.totalDocsExamined / Math.max(stats.nReturned, 1),
  }));
}

for (const [collection, field, id, expected] of [
  ["Stack", "config.server_id", manifest.monitor_server_id,
    manifest.server_relationship_counts.Stack],
  ["Deployment", "config.server_id", manifest.monitor_server_id,
    manifest.server_relationship_counts.Deployment],
  ["Repo", "config.server_id", manifest.monitor_server_id,
    manifest.server_relationship_counts.Repo],
  ["Stack", "config.swarm_id", manifest.monitor_swarm_id,
    manifest.swarm_relationship_counts.Stack],
  ["Deployment", "config.swarm_id", manifest.monitor_swarm_id,
    manifest.swarm_relationship_counts.Deployment],
]) {
  const actual = db.getCollection(collection).countDocuments({ [field]: id });
  if (expected <= 0 || actual !== expected) {
    throw new Error(
      collection + "." + field + " returned " + actual +
      ", expected positive count " + expected,
    );
  }
  explain(collection, { [field]: id });
}
for (const [operation, limit] of [["RunBuild", 2], ["CancelBuild", 1]]) {
  const filter = {
    "target.type": "Build",
    "target.id": manifest.latest_state_build_id,
    operation,
  };
  const expected = manifest.latest_state_operation_counts?.[operation];
  const actual = db.Update.countDocuments(filter);
  if (!Number.isInteger(expected) || expected < limit || actual !== expected) {
    throw new Error(
      operation + " returned " + actual +
      ", expected representative manifest count " + expected,
    );
  }
  explain("Update", filter, { start_ts: -1 }, limit);
}
explain("UserGroup", {
  $or: [
    { everyone: true },
    { users: manifest.user_id },
  ],
});
explain("Permission", {
  level: { $in: ["Read", "Execute", "Write"] },
});
for (const collection of [
  "Swarm", "Server", "Stack", "Deployment", "Build", "Repo",
  "Procedure", "Action", "ResourceSync", "Builder", "Alerter",
]) {
  explain(collection, {
    "base_permission.level": {
      $in: ["Read", "Execute", "Write"],
    },
  });
}
```

- [ ] **Step 3: Create the manifest-backed fixture validator**

Create `scripts/performance/validate-core-data-fixture.js`:

```javascript
const fs = require("fs");
const path = process.env.FIXTURE_MANIFEST;
if (!path) throw new Error("FIXTURE_MANIFEST is required");
const manifest = JSON.parse(fs.readFileSync(path, "utf8"));
const readable = ["Read", "Execute", "Write"];

function objectId(id, label) {
  if (!ObjectId.isValid(id)) throw new Error(label + " is not ObjectId");
  return ObjectId(id);
}
function assert(condition, message) {
  if (!condition) throw new Error(message);
}

const requiredCollections = [
  "Server", "Swarm", "Stack", "Deployment", "Build", "Repo",
  "Procedure", "Action",
];
for (const collection of requiredCollections) {
  assert(Number.isInteger(manifest.collection_counts?.[collection]),
    "missing exact collection_counts." + collection);
  const count = db.getCollection(collection).countDocuments({});
  assert(count === manifest.collection_counts[collection],
    collection + " count " + count + " != manifest " +
      manifest.collection_counts[collection]);
}
assert(manifest.permission_server_overhead === 1,
  "permission_server_overhead must be the one denied sentinel");
for (const collection of requiredCollections) {
  const expectedScaleCount = collection === "Server"
    ? manifest.fixture_size + manifest.permission_server_overhead
    : manifest.fixture_size;
  assert(manifest.collection_counts[collection] === expectedScaleCount,
    collection + " count must equal its scalable fixture count");
}

const user = db.User.findOne({ _id: objectId(manifest.user_id, "user_id") });
const group = db.UserGroup.findOne({
  _id: objectId(manifest.group_id, "group_id"),
});
assert(user && user.enabled && !user.admin, "fixture user is not enabled non-admin");
assert(group && (group.everyone || (group.users || []).includes(manifest.user_id)),
  "fixture user is not in fixture group");

const effectiveGroups = db.UserGroup.find({
  $or: [
    { everyone: true },
    { users: manifest.user_id },
  ],
}).toArray();
const effectiveGroupIds = effectiveGroups.map((row) => row._id.toHexString()).sort();
const declaredGroupIds = [...manifest.effective_group_ids].sort();
assert(EJSON.stringify(effectiveGroupIds) === EJSON.stringify(declaredGroupIds),
  "effective group membership differs from manifest");
assert(effectiveGroupIds.includes(manifest.group_id),
  "inherited grant group is not effective");

for (const [label, id] of Object.entries({
  direct_server_id: manifest.direct_server_id,
  inherited_server_id: manifest.inherited_server_id,
  denied_server_id: manifest.denied_server_id,
})) {
  assert(db.Server.countDocuments({ _id: objectId(id, label) }) === 1,
    label + " does not identify one Server");
}
assert(manifest.denied_server_id !== manifest.direct_server_id &&
    manifest.denied_server_id !== manifest.inherited_server_id,
  "denied Server must be the fixed sentinel, not a readable target");

function hasGrant(userType, userId, resourceId) {
  return db.Permission.countDocuments({
    "user_target.type": userType,
    "user_target.id": userId,
    "resource_target.type": "Server",
    "resource_target.id": resourceId,
    "level": { $in: readable },
  }) === 1;
}
assert(hasGrant("User", manifest.user_id, manifest.direct_server_id),
  "missing exact direct grant");
assert(hasGrant("UserGroup", manifest.group_id, manifest.inherited_server_id),
  "missing exact inherited grant");
assert(!hasGrant("User", manifest.user_id, manifest.denied_server_id),
  "denied target has direct grant");
assert(db.Permission.countDocuments({
  "user_target.type": "UserGroup",
  "user_target.id": { $in: effectiveGroupIds },
  "resource_target.type": "Server",
  "resource_target.id": manifest.denied_server_id,
  level: { $in: readable },
}) === 0, "denied target has an effective-group grant");

const denied = db.Server.findOne({
  _id: objectId(manifest.denied_server_id, "denied_server_id"),
});
assert(!denied.base_permission || !readable.includes(denied.base_permission.level),
  "denied target has readable base permission");
assert(!user.all || !user.all.Server || !readable.includes(user.all.Server.level),
  "fixture user has all-Server read");
assert(!group.all || !group.all.Server || !readable.includes(group.all.Server.level),
  "fixture group has all-Server read");
for (const effectiveGroup of effectiveGroups) {
  assert(!effectiveGroup.all || !effectiveGroup.all.Server ||
      !readable.includes(effectiveGroup.all.Server.level),
    "effective group has all-Server read: " + effectiveGroup._id);
}

for (const [collection, field, id, expected] of [
  ["Stack", "config.server_id", manifest.monitor_server_id,
    manifest.server_relationship_counts?.Stack],
  ["Deployment", "config.server_id", manifest.monitor_server_id,
    manifest.server_relationship_counts?.Deployment],
  ["Repo", "config.server_id", manifest.monitor_server_id,
    manifest.server_relationship_counts?.Repo],
  ["Stack", "config.swarm_id", manifest.monitor_swarm_id,
    manifest.swarm_relationship_counts?.Stack],
  ["Deployment", "config.swarm_id", manifest.monitor_swarm_id,
    manifest.swarm_relationship_counts?.Deployment],
]) {
  assert(Number.isInteger(expected) && expected > 0,
    "missing positive relationship count for " + collection + "." + field);
  assert(db.getCollection(collection).countDocuments({ [field]: id }) === expected,
    "relationship count mismatch for " + collection + "." + field);
}
assert(db.Server.countDocuments({
  _id: objectId(manifest.monitor_server_id, "monitor_server_id"),
}) === 1, "monitor_server_id does not identify one Server");
assert(db.Swarm.countDocuments({
  _id: objectId(manifest.monitor_swarm_id, "monitor_swarm_id"),
}) === 1, "monitor_swarm_id does not identify one Swarm");
assert(db.Build.countDocuments({
  _id: objectId(manifest.latest_state_build_id, "latest_state_build_id"),
}) === 1, "latest_state_build_id does not identify one Build");
for (const [operation, minimum] of [["RunBuild", 2], ["CancelBuild", 1]]) {
  const expected = manifest.latest_state_operation_counts?.[operation];
  assert(Number.isInteger(expected) && expected >= minimum,
    "missing representative latest-state count for " + operation);
  const actual = db.Update.countDocuments({
    "target.type": "Build",
    "target.id": manifest.latest_state_build_id,
    operation,
  });
  assert(actual === expected,
    operation + " count " + actual + " != manifest " + expected);
}

print(EJSON.stringify({
  fixture_size: manifest.fixture_size,
  permission_server_overhead: manifest.permission_server_overhead,
  user_id: manifest.user_id,
  group_id: manifest.group_id,
  direct_server_id: manifest.direct_server_id,
  inherited_server_id: manifest.inherited_server_id,
  denied_server_id: manifest.denied_server_id,
  effective_group_ids: effectiveGroupIds,
  collection_counts: manifest.collection_counts,
  server_relationship_counts: manifest.server_relationship_counts,
  swarm_relationship_counts: manifest.swarm_relationship_counts,
  latest_state_operation_counts: manifest.latest_state_operation_counts,
}));
```

Each staging database has its own untracked manifest JSON containing the IDs
above, the complete `effective_group_ids` set, exact counts for all eight
collections, positive Server/Swarm relationship counts, the representative
Build ID, and exact positive `RunBuild`/`CancelBuild` history counts. This
validator does not seed production-like data implicitly; the
fixture provisioning job must create the declared rows, then this script
proves the exact shape before any sample is accepted.

`fixture_size` remains the 1/100/1,000 scalable workload cardinality. Every
required collection has exactly that many workload rows except Server, which
has one additional denied permission sentinel declared by
`permission_server_overhead: 1`. At size 1 the direct and inherited readable
grants intentionally target the same one workload Server and the denied ID is
the extra sentinel; the validator requires those two IDs to differ. The
sentinel is excluded from readable endpoint results and makes permission
correctness possible without relabeling the scale.

- [ ] **Step 4: Create the isolated-staging profiler**

Write `scripts/performance/profile-core-data-paths.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

: "${KOMODO_ADDRESS:?set KOMODO_ADDRESS}"
: "${KOMODO_API_KEY:?set KOMODO_API_KEY}"
: "${KOMODO_API_SECRET:?set KOMODO_API_SECRET}"
: "${MONGODB_URI:?set MONGODB_URI selecting the fixture database}"
: "${P1_DATABASE_NAME:?set P1_DATABASE_NAME to the same p1_* database}"
: "${P1_PROFILE_USER_ID:?set P1_PROFILE_USER_ID to the manifest user}"
: "${FIXTURE_SIZE:?set FIXTURE_SIZE to 1, 100, or 1000}"
: "${FIXTURE_MANIFEST:?set FIXTURE_MANIFEST to the fixture JSON}"

case "$P1_DATABASE_NAME" in
  p1_*) ;;
  *) echo "P1_DATABASE_NAME must start with p1_" >&2; exit 2 ;;
esac
actual_database=$(mongosh "$MONGODB_URI" --quiet --eval 'print(db.getName())' | tail -1)
test "$actual_database" = "$P1_DATABASE_NAME" || {
  echo "mongosh selected $actual_database, expected $P1_DATABASE_NAME" >&2
  exit 2
}

case "${ENFORCE_BUDGET:-0}" in
  0|1) ;;
  *) echo "ENFORCE_BUDGET must be 0 or 1" >&2; exit 2 ;;
esac

actual_fixture_size=$(
  mongosh "$MONGODB_URI" --quiet --eval \
    'print(db.Server.countDocuments({}))' | tail -1
)
expected_server_count=$((FIXTURE_SIZE + 1))
test "$actual_fixture_size" = "$expected_server_count" || {
  echo "Server fixture count is $actual_fixture_size, expected $expected_server_count including denied sentinel" >&2
  exit 2
}

artifact="core-data-path-$(git rev-parse --short HEAD)-${FIXTURE_SIZE}.json"
work_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$work_dir"
}
trap cleanup EXIT HUP INT TERM

mongosh "$MONGODB_URI" --quiet \
  scripts/performance/validate-core-data-fixture.js \
  > "$work_dir/fixture.json"
manifest_user_id=$(jq -er '.user_id' "$work_dir/fixture.json")
test "$manifest_user_id" = "$P1_PROFILE_USER_ID" || {
  echo "P1_PROFILE_USER_ID does not match fixture manifest" >&2
  exit 2
}
api_user_id=$(
  curl --fail --silent --show-error \
    --header "x-api-key: $KOMODO_API_KEY" \
    --header "x-api-secret: $KOMODO_API_SECRET" \
    "$KOMODO_ADDRESS/user" | jq -er '._id["$oid"]'
)
test "$api_user_id" = "$P1_PROFILE_USER_ID" || {
  echo "HTTP API-key identity does not match fixture manifest" >&2
  exit 2
}

request() {
  local request_type="$1"
  curl --fail --silent --show-error --output /dev/null \
    --write-out '%{time_total}\n' \
    --header 'content-type: application/json' \
    --header "x-api-key: $KOMODO_API_KEY" \
    --header "x-api-secret: $KOMODO_API_SECRET" \
    --data "$2" \
    "$KOMODO_ADDRESS/read"
}

for _ in 1 2 3 4 5; do
  request ListUpdates '{"type":"ListUpdates","params":{"query":null,"page":0}}' >/dev/null
  request ListAlerts '{"type":"ListAlerts","params":{"query":null,"page":0}}' >/dev/null
done

for _ in $(seq 1 30); do
  request ListUpdates '{"type":"ListUpdates","params":{"query":null,"page":0}}'
done > "$work_dir/list-updates.txt"

for _ in $(seq 1 30); do
  request ListAlerts '{"type":"ListAlerts","params":{"query":null,"page":0}}'
done > "$work_dir/list-alerts.txt"

jq -n \
  --arg fixture_size "$FIXTURE_SIZE" \
  --argjson enforce "${ENFORCE_BUDGET:-0}" \
  --rawfile updates "$work_dir/list-updates.txt" \
  --rawfile alerts "$work_dir/list-alerts.txt" \
  'def percentile($p):
     sort as $values |
     $values[((($values | length) * $p | ceil) - 1)];
   ($updates | split("\n") | map(select(length > 0) | tonumber)) as $update_samples |
   ($alerts | split("\n") | map(select(length > 0) | tonumber)) as $alert_samples |
   ($update_samples | percentile(0.50)) as $update_p50 |
   ($update_samples | percentile(0.95)) as $update_p95 |
   ($alert_samples | percentile(0.50)) as $alert_p50 |
   ($alert_samples | percentile(0.95)) as $alert_p95 |
   if $enforce == 1 and
     ($update_p95 > 0.250 or $alert_p95 > 0.250) then
     error("warm endpoint exceeded p95 budget")
   else {
    fixture_size: ($fixture_size | tonumber),
    list_updates_seconds: $update_samples,
    list_alerts_seconds: $alert_samples,
    list_updates_p50_seconds: $update_p50,
    list_updates_p95_seconds: $update_p95,
    list_alerts_p50_seconds: $alert_p50,
    list_alerts_p95_seconds: $alert_p95
  } end' > "$artifact"

jq . "$artifact"
```

This HTTP script owns latency only: thirty post-warmup samples and nearest-rank
p50/p95. It deliberately does not infer resolver command work by subtracting a
different request while background loops are active. The isolated resolver
harness below owns command, examined-document, and response-byte gates.
Baseline runs leave `ENFORCE_BUDGET=0`; post-change gates set it to `1`, which
fails on p95 above 250 ms.

- [ ] **Step 4b: Add the authoritative isolated resolver command harness**

Register `#[cfg(test)] mod staging;` in `api/read/mod.rs`. In
`api/read/staging.rs`, add ignored test
`profile_list_endpoint_from_env`. It requires
`P1_ALLOW_MONGO_PROFILER=1`, an isolated resolved database name starting with
`p1_`, `P1_PROFILE_ENDPOINT=list_updates|list_alerts`,
`P1_PROFILE_MODE=warm`, `P1_PROFILE_USER_ID`, `P1_PROFILE_ARTIFACT`, and a
unique `KOMODO_DATABASE_APP_NAME`.

The test initializes only config/database state—never `CoreState`, monitor, or
refresh loops. Before the measured window it loads the current User and builds
`ReadArgs`, then performs five direct warmups through the real
`ListUpdates.resolve(&args)` or `ListAlerts.resolve(&args)`. It captures Mongo
server time, calls that same resolver once, captures end server time, and
queries `system.profile` strictly by the unique app name and closed timestamp
window. Count only read `find`, `aggregate`, `getMore`, `count`, and `distinct`
commands; record each matched profile row, namespaces, docs/keys examined,
response bytes, and total commands. Profiler setup/restore uses
`measure().await` followed by unconditional cleanup, preserving the exact prior
level on both success and returned error. The User lookup and profiler control
commands occur outside the timestamp window.

Create `profile-core-resolver-commands.sh` to run that ignored test in a fresh
Cargo test process for each endpoint and fixture. Each process gets a distinct
app name; the script validates the two artifacts with `jq` and, only when
`ENFORCE_BUDGET=1`, requires at most four commands per resolver. No
`GetVersion` subtraction is permitted. Run:

```bash
rtk env P1_ALLOW_MONGO_PROFILER=1 P1_PROFILE_MODE=warm P1_PROFILE_ENDPOINT=list_updates P1_PROFILE_USER_ID="$P1_PROFILE_USER_ID_1" P1_PROFILE_ARTIFACT=target/resolver-warm-updates-1.json KOMODO_DATABASE_URI="$MONGODB_URI_FIXTURE_1" KOMODO_DATABASE_DB_NAME="$P1_DATABASE_NAME_1" KOMODO_DATABASE_APP_NAME=p1-resolver-warm-updates-1 cargo test -p komodo_core --release api::read::staging::profile_list_endpoint_from_env -- --ignored --exact --nocapture --test-threads=1
rtk env P1_ALLOW_MONGO_PROFILER=1 P1_PROFILE_MODE=warm P1_PROFILE_ENDPOINT=list_alerts P1_PROFILE_USER_ID="$P1_PROFILE_USER_ID_1" P1_PROFILE_ARTIFACT=target/resolver-warm-alerts-1.json KOMODO_DATABASE_URI="$MONGODB_URI_FIXTURE_1" KOMODO_DATABASE_DB_NAME="$P1_DATABASE_NAME_1" KOMODO_DATABASE_APP_NAME=p1-resolver-warm-alerts-1 cargo test -p komodo_core --release api::read::staging::profile_list_endpoint_from_env -- --ignored --exact --nocapture --test-threads=1
rtk bash -n scripts/performance/profile-core-resolver-commands.sh
```

The wrapper supplies the corresponding untracked URI, explicit `p1_*` database
name, user ID, manifest, and fixture size for 1/100/1,000. Before launching,
it compares `mongosh ... db.getName()` with `KOMODO_DATABASE_DB_NAME` and
requires `P1_PROFILE_USER_ID` to equal the validator's `manifest.user_id`.
Baseline command artifacts are expected to expose
the P1; the post-change run enables the numeric gate.

- [ ] **Step 5: Make the profiler executable**

Run:

```bash
rtk chmod +x scripts/performance/profile-core-data-paths.sh
rtk chmod +x scripts/performance/profile-core-resolver-commands.sh
```

Expected: exit 0.

- [ ] **Step 6: Verify the scripts and fixture without credentials**

Run:

```bash
rtk bash -n scripts/performance/profile-core-data-paths.sh
rtk bash -n scripts/performance/profile-core-resolver-commands.sh
```

Expected: exit 0 and no output.

Run:

```bash
rtk env FIXTURE_MANIFEST="$FIXTURE_MANIFEST_1" mongosh "$MONGODB_URI_FIXTURE_1" --quiet scripts/performance/validate-core-data-fixture.js
```

Expected: one manifest summary and exit 0. Repeat for the 100 and 1,000
manifests before collecting data.

Run: `rtk env FIXTURE_MANIFEST="$FIXTURE_MANIFEST_1000" mongosh "$MONGODB_URI_FIXTURE_1000" --quiet scripts/performance/inspect-core-data-indexes.js`

Expected: one JSON `indexes` record for each named collection and `explain` records for every fixture target that exists. Save this output before adding indexes.

Capture production metadata separately before cloning/staging work:

```bash
rtk mkdir -p target
rtk mongosh "$MONGODB_URI_PRODUCTION" --quiet scripts/performance/inventory-core-data-indexes.js > "target/core-data-production-indexes-$(rtk git rev-parse --short HEAD).jsonl"
```

Expected: a non-empty read-only artifact with all fourteen collection records and
no `explain` record. Do not echo or commit the URI.

- [ ] **Step 7: Capture all three pre-change samples**

Run:

```bash
rtk env FIXTURE_SIZE=1 FIXTURE_MANIFEST="$FIXTURE_MANIFEST_1" MONGODB_URI="$MONGODB_URI_FIXTURE_1" P1_DATABASE_NAME="$P1_DATABASE_NAME_1" P1_PROFILE_USER_ID="$P1_PROFILE_USER_ID_1" KOMODO_ADDRESS="$KOMODO_ADDRESS_FIXTURE_1" scripts/performance/profile-core-data-paths.sh
rtk env FIXTURE_SIZE=100 FIXTURE_MANIFEST="$FIXTURE_MANIFEST_100" MONGODB_URI="$MONGODB_URI_FIXTURE_100" P1_DATABASE_NAME="$P1_DATABASE_NAME_100" P1_PROFILE_USER_ID="$P1_PROFILE_USER_ID_100" KOMODO_ADDRESS="$KOMODO_ADDRESS_FIXTURE_100" scripts/performance/profile-core-data-paths.sh
rtk env FIXTURE_SIZE=1000 FIXTURE_MANIFEST="$FIXTURE_MANIFEST_1000" MONGODB_URI="$MONGODB_URI_FIXTURE_1000" P1_DATABASE_NAME="$P1_DATABASE_NAME_1000" P1_PROFILE_USER_ID="$P1_PROFILE_USER_ID_1000" KOMODO_ADDRESS="$KOMODO_ADDRESS_FIXTURE_1000" scripts/performance/profile-core-data-paths.sh
rtk env ENFORCE_BUDGET=0 scripts/performance/profile-core-resolver-commands.sh
```

Each address is an otherwise identical Core pointed at the correspondingly
named staging database; do not relabel one unchanged database three times.
Expected: three JSON artifacts with 30 `ListUpdates` and 30 `ListAlerts`
timings plus six isolated resolver-command artifacts. The pre-change Mongo
command count may exceed the budget; that is the reproduced P1 and must not be
“fixed” in the baseline script.

- [ ] **Step 8: Commit the reproducible baseline protocol**

```bash
rtk git add docs/performance/core-data-path-budgets.md scripts/performance/inventory-core-data-indexes.js scripts/performance/inspect-core-data-indexes.js scripts/performance/validate-core-data-fixture.js scripts/performance/profile-core-data-paths.sh scripts/performance/profile-core-resolver-commands.sh bin/core/src/api/read/mod.rs bin/core/src/api/read/staging.rs
rtk git commit -m "perf: define core data path budgets"
```

Expected: one commit containing only the budget and measurement files; raw artifacts remain untracked.

### Task 2: Reconcile only verified missing MongoDB indexes

**Files:**
- Create: `lib/database/src/indexes.rs`
- Modify: `lib/database/src/lib.rs:29-113,245-255`
- Test: `lib/database/src/indexes.rs`

- [ ] **Step 1: Write the failing key-pattern test**

Create `lib/database/src/indexes.rs` with only this test:

```rust
#[cfg(test)]
mod tests {
  use crate::bson::doc;
  use mungos::mongodb::{IndexModel, options::IndexOptions};

  use super::{index_is_reusable, same_key_pattern};

  #[test]
  fn index_key_order_is_part_of_identity() {
    assert!(same_key_pattern(
      &doc! { "target.type": 1, "target.id": 1 },
      &doc! { "target.type": 1, "target.id": 1 },
    ));
    assert!(!same_key_pattern(
      &doc! { "target.type": 1, "target.id": 1 },
      &doc! { "target.id": 1, "target.type": 1 },
    ));
  }

  #[test]
  fn partial_or_hidden_index_is_not_reused() {
    let partial = IndexModel::builder()
      .keys(doc! { "target.type": 1 })
      .options(
        IndexOptions::builder()
          .partial_filter_expression(doc! { "operation": "RunBuild" })
          .build(),
      )
      .build();
    assert!(!index_is_reusable(&partial));
  }
}
```

Add `mod indexes;` at `lib/database/src/lib.rs:43`.

- [ ] **Step 2: Run the focused test to verify RED**

Run: `rtk cargo test -p database indexes::tests::index_key_order_is_part_of_identity -- --exact`

Expected: FAIL with unresolved import/function `same_key_pattern`.

- [ ] **Step 3: Implement idempotent key reconciliation**

Place this implementation above the test in `lib/database/src/indexes.rs`:

```rust
use anyhow::{Context, bail};
use futures_util::TryStreamExt;
use mungos::mongodb::{
  Collection, IndexModel,
  bson::Document,
  options::IndexOptions,
};

pub fn same_key_pattern(left: &Document, right: &Document) -> bool {
  left.iter().eq(right.iter())
}

pub fn index_is_reusable(index: &IndexModel) -> bool {
  let Some(options) = &index.options else {
    return true;
  };
  options.sparse != Some(true)
    && options.partial_filter_expression.is_none()
    && options.hidden != Some(true)
    && options.collation.is_none()
}

pub async fn ensure_index<T: Send + Sync>(
  collection: &Collection<T>,
  name: &str,
  keys: Document,
) -> anyhow::Result<bool> {
  let mut indexes = collection
    .list_indexes()
    .await
    .with_context(|| format!("failed to list indexes for {name}"))?;
  while let Some(index) = indexes.try_next().await? {
    if same_key_pattern(&index.keys, &keys) {
      if index_is_reusable(&index) {
        return Ok(false);
      }
      bail!(
        "index {name} has the required keys but incompatible sparse, partial, hidden, or collation options"
      );
    }
  }
  collection
    .create_index(
      IndexModel::builder()
        .keys(keys)
        .options(
          IndexOptions::builder()
            .name(name.to_string())
            .build(),
        )
        .build(),
    )
    .await
    .with_context(|| format!("failed to create index {name}"))?;
  Ok(true)
}

#[cfg(test)]
mod tests {
  use crate::bson::doc;
  use mungos::mongodb::{IndexModel, options::IndexOptions};

  use super::{index_is_reusable, same_key_pattern};

  #[test]
  fn index_key_order_is_part_of_identity() {
    assert!(same_key_pattern(
      &doc! { "target.type": 1, "target.id": 1 },
      &doc! { "target.type": 1, "target.id": 1 },
    ));
    assert!(!same_key_pattern(
      &doc! { "target.type": 1, "target.id": 1 },
      &doc! { "target.id": 1, "target.type": 1 },
    ));
  }

  #[test]
  fn partial_or_hidden_index_is_not_reused() {
    let partial = IndexModel::builder()
      .keys(doc! { "target.type": 1 })
      .options(
        IndexOptions::builder()
          .partial_filter_expression(doc! { "operation": "RunBuild" })
          .build(),
      )
      .build();
    assert!(!index_is_reusable(&partial));
  }
}
```

- [ ] **Step 4: Run the focused test to verify GREEN**

Run: `rtk cargo test -p database indexes::tests::index_key_order_is_part_of_identity -- --exact`

Expected: PASS.

- [ ] **Step 5: Register the exact relationship and latest-state indexes**

At `lib/database/src/lib.rs:29` import `ensure_index`:

```rust
use indexes::ensure_index;
```

After constructing `client` and before `Ok(client)` at `lib/database/src/lib.rs:112`, add:

```rust
ensure_index(
  &client.stacks,
  "stack_server_id",
  doc! { "config.server_id": 1 },
)
.await?;
ensure_index(
  &client.stacks,
  "stack_swarm_id",
  doc! { "config.swarm_id": 1 },
)
.await?;
ensure_index(
  &client.deployments,
  "deployment_server_id",
  doc! { "config.server_id": 1 },
)
.await?;
ensure_index(
  &client.deployments,
  "deployment_swarm_id",
  doc! { "config.swarm_id": 1 },
)
.await?;
ensure_index(
  &client.repos,
  "repo_server_id",
  doc! { "config.server_id": 1 },
)
.await?;
ensure_index(
  &client.user_groups,
  "user_group_everyone",
  doc! { "everyone": 1 },
)
.await?;
ensure_index(
  &client.user_groups,
  "user_group_users",
  doc! { "users": 1 },
)
.await?;
ensure_index(
  &client.permissions,
  "permission_level",
  doc! { "level": 1 },
)
.await?;
macro_rules! ensure_base_permission_index {
  ($collection:expr, $name:literal) => {
    ensure_index(
      $collection,
      $name,
      doc! { "base_permission.level": 1 },
    )
    .await?;
  };
}
ensure_base_permission_index!(&client.swarms, "swarm_base_permission");
ensure_base_permission_index!(&client.servers, "server_base_permission");
ensure_base_permission_index!(&client.stacks, "stack_base_permission");
ensure_base_permission_index!(
  &client.deployments,
  "deployment_base_permission"
);
ensure_base_permission_index!(&client.builds, "build_base_permission");
ensure_base_permission_index!(&client.repos, "repo_base_permission");
ensure_base_permission_index!(
  &client.procedures,
  "procedure_base_permission"
);
ensure_base_permission_index!(&client.actions, "action_base_permission");
ensure_base_permission_index!(
  &client.resource_syncs,
  "resource_sync_base_permission"
);
ensure_base_permission_index!(&client.builders, "builder_base_permission");
ensure_base_permission_index!(&client.alerters, "alerter_base_permission");
ensure_index(
  &client.updates,
  "update_target_operation_start",
  doc! {
    "target.type": 1,
    "target.id": 1,
    "operation": 1,
    "start_ts": -1,
  },
)
.await?;
```

The Permission-level and eleven base-permission indexes prevent the shared
generation input loader from scanning unreadable rows in every collection.
The two UserGroup indexes cover both branches of
`$or: [{ everyone: true }, { users: user_id }]`; `users` is a normal multikey
index. The helper deliberately reuses a differently named manual index only when its
key order matches and it is neither sparse, partial, hidden, nor collated. An
incompatible same-key index stops startup with an actionable error so an
operator can review it; do not silently treat it as query coverage and do not
add duplicate `doc_index` attributes to `Update`.

- [ ] **Step 6: Run database and Core tests**

Run: `rtk cargo test -p database`

Expected: all database tests PASS.

Run: `rtk cargo test -p komodo_core`

Expected: all Core tests PASS.

- [ ] **Step 7: Verify index creation on a staging clone**

Start one new Core against the staging clone, then run:

```bash
rtk env FIXTURE_MANIFEST="$FIXTURE_MANIFEST" mongosh "$MONGODB_URI" --quiet scripts/performance/inspect-core-data-indexes.js
```

Expected: the five monitor relationship patterns, two UserGroup membership
patterns, Permission level, eleven resource base-permission patterns, and
`update_target_operation_start` exist exactly once. The
UserGroup `$or` winning plan contains an indexed OR with both branches covered;
all representative queries report `IXSCAN` and ratio <= 5. If staging already
had an equivalent key pattern under another name, startup logs no conflict and
does not create a duplicate.

- [ ] **Step 8: Commit the index reconciliation**

```bash
rtk git add lib/database/src/indexes.rs lib/database/src/lib.rs
rtk git commit -m "perf: index core relationship queries"
```

Expected: one focused database commit.

### Task 3: Close checkpoint 1 with cold-start and rollback proof

**Files:**
- Modify: `docs/performance/core-data-path-budgets.md`

- [ ] **Step 1: Verify an empty-cache Core still starts**

Run: `rtk cargo run -p komodo_core --bin core` against the isolated staging MongoDB.

Expected: Core reaches its listening state and index reconciliation completes without duplicate-index errors.

- [ ] **Step 2: Record the index rollback commands**

Append this exact section to `docs/performance/core-data-path-budgets.md`:

```markdown
## Index rollback

Disable any code that relies on a new index before dropping it. On the staging clone:

    rtk mongosh "$MONGODB_URI" --quiet --eval 'for (const [c, n] of [["Stack","stack_server_id"],["Stack","stack_swarm_id"],["Deployment","deployment_server_id"],["Deployment","deployment_swarm_id"],["Repo","repo_server_id"],["UserGroup","user_group_everyone"],["UserGroup","user_group_users"],["Permission","permission_level"],["Swarm","swarm_base_permission"],["Server","server_base_permission"],["Stack","stack_base_permission"],["Deployment","deployment_base_permission"],["Build","build_base_permission"],["Repo","repo_base_permission"],["Procedure","procedure_base_permission"],["Action","action_base_permission"],["ResourceSync","resource_sync_base_permission"],["Builder","builder_base_permission"],["Alerter","alerter_base_permission"],["Update","update_target_operation_start"]]) { if (db.getCollection(c).getIndexes().some(i => i.name === n)) db.getCollection(c).dropIndex(n); }'

Re-run `scripts/performance/inspect-core-data-indexes.js` with the same
`FIXTURE_MANIFEST` after rollback. Never drop a differently named pre-existing
manual index.
```

- [ ] **Step 3: Run format and full checkpoint tests**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo test -p database
rtk cargo test -p komodo_core
```

Expected: all three commands exit 0.

- [ ] **Step 4: Commit the checkpoint runbook**

```bash
rtk git add docs/performance/core-data-path-budgets.md
rtk git commit -m "docs: add core index rollback"
```

Expected: checkpoint 1 is independently deployable. Attach pre/post `inspect-core-data-indexes.js` output to the PR.

### Task 4: Group monitoring resources with fixed collection-sized reads

**Files:**
- Create: `bin/core/src/monitor/inventory.rs`
- Modify: `bin/core/src/monitor/mod.rs:38-80,103-128,259-325`
- Modify: `bin/core/src/monitor/swarm.rs:46-60,77-99`
- Create: `scripts/performance/profile-core-background-cycles.sh`
- Create: `scripts/performance/capture-core-background-profile.js`
- Test: `bin/core/src/monitor/inventory.rs`

- [ ] **Step 1: Write the failing grouping test**

Create `bin/core/src/monitor/inventory.rs` with:

```rust
#[cfg(test)]
mod tests {
  use komodo_client::entities::{
    deployment::Deployment, repo::Repo, stack::Stack,
  };

  use super::MonitoringInventory;

  #[test]
  fn groups_resources_by_monitor_target() {
    let mut stack = Stack::default();
    stack.id = "stack-1".into();
    stack.config.server_id = "server-1".into();
    let mut deployment = Deployment::default();
    deployment.id = "deployment-1".into();
    deployment.config.swarm_id = "swarm-1".into();
    let mut repo = Repo::default();
    repo.id = "repo-1".into();
    repo.config.server_id = "server-1".into();

    let inventory = MonitoringInventory::from_resources(
      vec![stack],
      vec![deployment],
      vec![repo],
    );

    assert_eq!(inventory.servers["server-1"].stacks.len(), 1);
    assert_eq!(inventory.servers["server-1"].repos.len(), 1);
    assert_eq!(inventory.swarms["swarm-1"].deployments.len(), 1);
  }
}
```

Add `mod inventory;` next to `mod resources;` at `bin/core/src/monitor/mod.rs:41`.

- [ ] **Step 2: Run the grouping test to verify RED**

Run: `rtk cargo test -p komodo_core monitor::inventory::tests::groups_resources_by_monitor_target -- --exact`

Expected: FAIL because `MonitoringInventory` is not defined.

- [ ] **Step 3: Implement the inventory and collection-sized loaders**

Place this code above the test in `bin/core/src/monitor/inventory.rs`:

```rust
use std::collections::HashMap;

use database::{bson::doc, mungos::find::find_collect};
use komodo_client::entities::{
  deployment::Deployment, repo::Repo, server::Server,
  stack::Stack, swarm::Swarm,
};
use tracing::error;

use crate::state::db_client;

#[derive(Default)]
pub(super) struct RefreshCacheResources {
  pub stacks: Vec<Stack>,
  pub deployments: Vec<Deployment>,
  pub repos: Vec<Repo>,
}

#[derive(Default)]
pub(super) struct MonitoringInventory {
  pub servers: HashMap<String, RefreshCacheResources>,
  pub swarms: HashMap<String, RefreshCacheResources>,
}

fn rows_or_default<T, E>(
  result: Result<Vec<T>, E>,
  scope: &str,
  collection: &str,
) -> Vec<T>
where
  E: std::fmt::Display,
{
  result
    .inspect_err(|e| error!(
      "failed to load {scope} monitoring {collection} | {e:#}"
    ))
    .unwrap_or_default()
}

impl MonitoringInventory {
  pub fn from_resources(
    stacks: Vec<Stack>,
    deployments: Vec<Deployment>,
    repos: Vec<Repo>,
  ) -> Self {
    let mut inventory = Self::default();
    for stack in stacks {
      if !stack.config.server_id.is_empty() {
        inventory
          .servers
          .entry(stack.config.server_id.clone())
          .or_default()
          .stacks
          .push(stack.clone());
      }
      if !stack.config.swarm_id.is_empty() {
        inventory
          .swarms
          .entry(stack.config.swarm_id.clone())
          .or_default()
          .stacks
          .push(stack);
      }
    }
    for deployment in deployments {
      if !deployment.config.server_id.is_empty() {
        inventory
          .servers
          .entry(deployment.config.server_id.clone())
          .or_default()
          .deployments
          .push(deployment.clone());
      }
      if !deployment.config.swarm_id.is_empty() {
        inventory
          .swarms
          .entry(deployment.config.swarm_id.clone())
          .or_default()
          .deployments
          .push(deployment);
      }
    }
    for repo in repos {
      if !repo.config.server_id.is_empty() {
        inventory
          .servers
          .entry(repo.config.server_id.clone())
          .or_default()
          .repos
          .push(repo);
      }
    }
    inventory
  }

  fn from_results<E1, E2, E3>(
    stacks: Result<Vec<Stack>, E1>,
    deployments: Result<Vec<Deployment>, E2>,
    repos: Result<Vec<Repo>, E3>,
    scope: &str,
  ) -> Self
  where
    E1: std::fmt::Display,
    E2: std::fmt::Display,
    E3: std::fmt::Display,
  {
    Self::from_resources(
      rows_or_default(stacks, scope, "stacks"),
      rows_or_default(deployments, scope, "deployments"),
      rows_or_default(repos, scope, "repos"),
    )
  }

  pub async fn load_for_servers() -> Self {
    let (stacks, deployments, repos) = tokio::join!(
      find_collect(&db_client().stacks, None, None),
      find_collect(&db_client().deployments, None, None),
      find_collect(&db_client().repos, None, None),
    );
    Self::from_results(stacks, deployments, repos, "global server")
  }

  pub async fn load_for_swarms() -> Self {
    let (stacks, deployments) = tokio::join!(
      find_collect(&db_client().stacks, None, None),
      find_collect(&db_client().deployments, None, None),
    );
    Self::from_resources(
      rows_or_default(stacks, "global swarm", "stacks"),
      rows_or_default(
        deployments,
        "global swarm",
        "deployments",
      ),
      Vec::new(),
    )
  }
}

impl RefreshCacheResources {
  pub async fn load_server(server: &Server) -> Self {
    let (stacks, deployments, repos) = tokio::join!(
      find_collect(
        &db_client().stacks,
        doc! { "config.server_id": &server.id },
        None,
      ),
      find_collect(
        &db_client().deployments,
        doc! { "config.server_id": &server.id },
        None,
      ),
      find_collect(
        &db_client().repos,
        doc! { "config.server_id": &server.id },
        None,
      ),
    );
    Self {
      stacks: stacks
        .inspect_err(|e| error!(
          "failed to load targeted server stacks | server: {} | {e:#}",
          server.name,
        ))
        .unwrap_or_default(),
      deployments: deployments
        .inspect_err(|e| error!(
          "failed to load targeted server deployments | server: {} | {e:#}",
          server.name,
        ))
        .unwrap_or_default(),
      repos: repos
        .inspect_err(|e| error!(
          "failed to load targeted server repos | server: {} | {e:#}",
          server.name,
        ))
        .unwrap_or_default(),
    }
  }

  pub async fn load_swarm(swarm: &Swarm) -> Self {
    let (stacks, deployments) = tokio::join!(
      find_collect(
        &db_client().stacks,
        doc! { "config.swarm_id": &swarm.id },
        None,
      ),
      find_collect(
        &db_client().deployments,
        doc! { "config.swarm_id": &swarm.id },
        None,
      ),
    );
    Self {
      stacks: stacks
        .inspect_err(|e| error!(
          "failed to load targeted swarm stacks | swarm: {} | {e:#}",
          swarm.name,
        ))
        .unwrap_or_default(),
      deployments: deployments
        .inspect_err(|e| error!(
          "failed to load targeted swarm deployments | swarm: {} | {e:#}",
          swarm.name,
        ))
        .unwrap_or_default(),
      repos: Vec::new(),
    }
  }
}
```

Keep the test below this implementation.

- [ ] **Step 4: Run the grouping test to verify GREEN**

Run: `rtk cargo test -p komodo_core monitor::inventory::tests::groups_resources_by_monitor_target -- --exact`

Expected: PASS.

- [ ] **Step 5: Feed one inventory into the server loop**

At `bin/core/src/monitor/mod.rs`:

- import `inventory::{MonitoringInventory, RefreshCacheResources}`;
- delete the old `RefreshCacheResources` definition and `load_server`/`load_swarm` implementation at lines 259–318;
- change `refresh_all_server_cache` to:

```rust
async fn refresh_all_server_cache(ts: i64) {
  let servers =
    match find_collect(&db_client().servers, None, None).await {
      Ok(servers) => servers,
      Err(e) => {
        error!(
          "Failed to get server list (refresh server cache) | {e:#}"
        );
        return;
      }
    };
  let mut inventory =
    MonitoringInventory::load_for_servers().await.servers;
  let futures = servers.into_iter().map(|server| {
    let resources = inventory.remove(&server.id).unwrap_or_default();
    async move {
      refresh_server_cache_controlled(
        &server,
        false,
        Some(resources),
      )
        .await;
    }
  });
  join_all(futures).await;
  tokio::join!(check_alerts(ts), record_server_stats(ts));
}
```

Keep the public `refresh_server_cache(server, force)` explicit
`fn -> impl Future + Send` signature and its `manual_async_fn` allowance. The
current source needs that form because `periphery_client` can call back into the
same function. It delegates without preloading:

```rust
#[allow(clippy::manual_async_fn)]
pub fn refresh_server_cache(
  server: &Server,
  force: bool,
) -> impl Future<Output = ()> + Send + '_ {
  refresh_server_cache_controlled(server, force, None)
}
```

Rename the current implementation at `monitor/mod.rs:103-257` to the private
`refresh_server_cache_controlled`, add
`preloaded: Option<RefreshCacheResources>`, and retain the same explicit
future shape. Inside its existing `async move`, acquire the controller, handle
the busy path, apply the one-second early return, and update `*lock` **before**
loading. Only then replace the old load with:

```rust
let resources = match preloaded {
  Some(resources) => resources,
  None => RefreshCacheResources::load_server(server).await,
};
```

The controller lock remains held across the selected refresh exactly as today.
A skipped call performs zero inventory queries, and a forced call that waits
does not carry a stale preloaded targeted inventory. The controller,
disabled/error paths, `PollStatus`, cache writes, and repo status loop otherwise
remain unchanged. Keep the existing `insert_status_unknown` impl in
`monitor/mod.rs`; only the two loaders move to `inventory.rs` as shown in Step
3. Add tests with an injected loader counter: busy and sub-second non-forced
calls observe zero loads, while an admitted direct call observes exactly one.

- [ ] **Step 6: Feed one inventory into the swarm loop**

Change `refresh_all_swarm_cache` in `bin/core/src/monitor/swarm.rs` to load
`MonitoringInventory::load_for_swarms()` once and pass each
`inventory.swarms.remove(&swarm.id).unwrap_or_default()` as `Some(resources)`
to `refresh_swarm_cache_controlled`. Rename the current implementation at
`monitor/swarm.rs:77-180`, add
`preloaded: Option<RefreshCacheResources>`, and move the selection shown above
to immediately after controller admission and the one-second check. Keep
`pub async fn refresh_swarm_cache(swarm, force)` as a wrapper that calls the
controlled function with `None`; it must not load before the controller. Move
the existing `load_swarm` implementation from
`monitor/mod.rs:296-318` into `inventory.rs` with its exact Stack and
Deployment `config.swarm_id` filters. Add the same zero-load busy/recent tests
for Swarm.

Both global loaders use `tokio::join!`, not `try_join!`. Preserve the current
partial-error behavior: if one collection query fails, log that failure and
group the other successful collections instead of discarding the entire
monitor cycle. Add a small `from_results` reducer test in `inventory.rs` that
injects a failed Stack result and proves Deployment and Repo entries survive.

- [ ] **Step 7: Run the monitoring tests and Core suite**

Run:

```bash
rtk cargo test -p komodo_core monitor::inventory
rtk cargo test -p komodo_core
```

Expected: the inventory test and the complete Core suite PASS.

- [ ] **Step 8: Measure the fixed query count**

Refactor the production server/swarm input loads into one-call helpers also
used by ignored, environment-gated tests
`profile_server_inventory_once`/`profile_swarm_inventory_once`. Add
`FindOptions.comment` to every list/inventory query with these prefixes:

- `p1:server-inventory:{servers,stacks,deployments,repos}`;
- `p1:swarm-inventory:{swarms,stacks,deployments}`.

The ignored tests call the real helpers against `KOMODO_DATABASE_URI`; they do
not contact Periphery. Create
`scripts/performance/capture-core-background-profile.js`:

```javascript
const prefix = process.env.PROBE_PREFIX;
const expectedLogical = Number(process.env.EXPECTED_LOGICAL);
const maxGetmore = Number(process.env.MAX_GETMORE);
const maxResponseBytes = Number(process.env.MAX_RESPONSE_BYTES);
if (!prefix) throw new Error("PROBE_PREFIX is required");

const match = {
  $or: [
    { "command.comment": { $regex: "^" + prefix } },
    { "originatingCommand.comment": { $regex: "^" + prefix } },
  ],
};
const rows = db.system.profile.aggregate([
  { $match: match },
  {
    $group: {
      _id: null,
      logical: {
        $sum: {
          $cond: [
            { $or: [
              { $ne: [{ $type: "$command.find" }, "missing"] },
              { $ne: [{ $type: "$command.aggregate" }, "missing"] },
            ] },
            1,
            0,
          ],
        },
      },
      getmore: {
        $sum: { $cond: [
          { $ne: [{ $type: "$command.getMore" }, "missing"] },
          1,
          0,
        ] },
      },
      docs_examined: { $sum: { $ifNull: ["$docsExamined", 0] } },
      keys_examined: { $sum: { $ifNull: ["$keysExamined", 0] } },
      response_bytes: { $sum: { $ifNull: ["$responseLength", 0] } },
      millis: { $sum: { $ifNull: ["$millis", 0] } },
    },
  },
]).toArray();
if (rows.length !== 1) throw new Error("no unique profiler group for " + prefix);
const row = rows[0];
if (row.logical !== expectedLogical) {
  throw new Error(prefix + " logical count " + row.logical);
}
if (row.getmore > maxGetmore) throw new Error(prefix + " getMore budget");
if (row.response_bytes > maxResponseBytes) {
  throw new Error(prefix + " response-byte budget");
}
print(EJSON.stringify({ prefix, ...row }));
```

Create `scripts/performance/profile-core-background-cycles.sh` with this exact
portable content. Commands inside the committed script are plain commands;
`rtk` is used only by the outer operator invocations in this plan.

```sh
#!/usr/bin/env sh
set -eu

: "${MONGODB_URI:?set MONGODB_URI selecting the fixture database}"
: "${FIXTURE_MANIFEST:?set FIXTURE_MANIFEST to the fixture JSON}"
: "${FIXTURE_SIZE:?set FIXTURE_SIZE to 1, 100, or 1000}"

case "$FIXTURE_SIZE" in
  1|100|1000) ;;
  *) echo "FIXTURE_SIZE must be 1, 100, or 1000" >&2; exit 2 ;;
esac

for tool in cargo cat env git jq mongosh mktemp rm tail tr wc; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "missing required command: $tool" >&2
    exit 2
  }
done
test -r "$FIXTURE_MANIFEST" || {
  echo "fixture manifest is not readable: $FIXTURE_MANIFEST" >&2
  exit 2
}

manifest_size=$(jq -er '.fixture_size' "$FIXTURE_MANIFEST")
test "$manifest_size" = "$FIXTURE_SIZE" || {
  echo "manifest fixture_size is $manifest_size, expected $FIXTURE_SIZE" >&2
  exit 2
}

original_profile_level=$(
  mongosh "$MONGODB_URI" --quiet --eval \
    'print(db.getProfilingStatus().was)' | tail -1
)
test "$original_profile_level" = 0 || {
  echo "refusing to replace non-zero Mongo profiler level" >&2
  exit 2
}

work_dir=$(mktemp -d)
artifact="core-background-$(git rev-parse --short HEAD)-${FIXTURE_SIZE}.jsonl"
: > "$artifact"

cleanup() {
  mongosh "$MONGODB_URI" --quiet --eval \
    'db.setProfilingLevel(0)' >/dev/null 2>&1 || true
  rm -rf "$work_dir"
}
trap cleanup 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

env FIXTURE_MANIFEST="$FIXTURE_MANIFEST" \
  mongosh "$MONGODB_URI" --quiet \
  scripts/performance/validate-core-data-fixture.js \
  > "$work_dir/fixture.json"
validated_size=$(jq -er '.fixture_size' "$work_dir/fixture.json")
test "$validated_size" = "$FIXTURE_SIZE" || {
  echo "validated fixture_size is $validated_size, expected $FIXTURE_SIZE" >&2
  exit 2
}

disable_profiler() {
  mongosh "$MONGODB_URI" --quiet --eval \
    'db.setProfilingLevel(0)' >/dev/null
}

reset_profiler() {
  disable_profiler
  mongosh "$MONGODB_URI" --quiet --eval \
    'db.system.profile.drop(); db.setProfilingLevel(2, { slowms: 0 })' \
    >/dev/null
}

probe_count=0
run_probe() {
  test_name=$1
  prefix=$2
  logical=$3
  getmore=$4
  bytes=$5

  reset_profiler
  env KOMODO_DATABASE_URI="$MONGODB_URI" \
    P1_FIXTURE_SIZE="$FIXTURE_SIZE" \
    cargo test -p komodo_core "$test_name" -- --ignored --exact
  disable_profiler
  env PROBE_PREFIX="$prefix" EXPECTED_LOGICAL="$logical" \
    MAX_GETMORE="$getmore" MAX_RESPONSE_BYTES="$bytes" \
    mongosh "$MONGODB_URI" --quiet \
    scripts/performance/capture-core-background-profile.js \
    >> "$artifact"
  probe_count=$((probe_count + 1))
}

run_probe \
  'monitor::inventory::tests::profile_server_inventory_once' \
  'p1:server-inventory:' 4 12 67108864
run_probe \
  'monitor::inventory::tests::profile_swarm_inventory_once' \
  'p1:swarm-inventory:' 3 8 67108864

artifact_lines=$(wc -l < "$artifact" | tr -d '[:space:]')
test "$artifact_lines" = "$probe_count" || {
  echo "artifact has $artifact_lines rows, expected $probe_count" >&2
  exit 2
}
cat "$artifact"
```

The wrapper initially runs the two real monitoring input probes. Task 5 adds
four state probes to the same `run_probe` list. Each successful probe appends
exactly one JSON line to `core-background-<sha>-<fixture>.jsonl`; any shell,
test, capture, or signal exit still disables Mongo profiling.

Run:

```bash
rtk chmod +x scripts/performance/profile-core-background-cycles.sh
rtk bash -n scripts/performance/profile-core-background-cycles.sh
rtk env FIXTURE_SIZE=1000 FIXTURE_MANIFEST="$FIXTURE_MANIFEST_1000" MONGODB_URI="$MONGODB_URI_FIXTURE_1000" scripts/performance/profile-core-background-cycles.sh
```

Expected: server reports exactly four logical finds and swarm exactly three;
cursor/getMore, examined work, bytes, and milliseconds remain visible rather
than being mislabeled as extra logical queries.

- [ ] **Step 9: Commit monitoring batching**

```bash
rtk git add bin/core/src/monitor/inventory.rs bin/core/src/monitor/mod.rs bin/core/src/monitor/swarm.rs scripts/performance/profile-core-background-cycles.sh scripts/performance/capture-core-background-profile.js
rtk git commit -m "perf: batch monitoring inventory reads"
```

Expected: one commit that does not change monitoring status semantics.

### Task 5: Replace per-resource latest-state reads with one aggregation per type

**Files:**
- Create: `bin/core/src/helpers/latest_states.rs`
- Create: `scripts/performance/explain-latest-state-aggregations.js`
- Modify: `bin/core/src/helpers/mod.rs`
- Modify: `bin/core/src/resource/build.rs:229-253,316-380`
- Modify: `bin/core/src/resource/repo.rs:207-232,293-325`
- Modify: `bin/core/src/resource/procedure.rs:369-426`
- Modify: `bin/core/src/resource/action.rs:163-220`
- Test: `bin/core/src/helpers/latest_states.rs`

- [ ] **Step 1: Write the failing Build cancellation reducer test**

Create `bin/core/src/helpers/latest_states.rs` with:

```rust
#[cfg(test)]
mod tests {
  use komodo_client::entities::{
    Operation, build::BuildState,
  };

  use super::{StateUpdate, build_state};

  #[test]
  fn cancel_after_latest_build_uses_previous_build() {
    let updates = vec![
      StateUpdate {
        operation: Operation::RunBuild,
        start_ts: 300,
        success: false,
      },
      StateUpdate {
        operation: Operation::CancelBuild,
        start_ts: 350,
        success: true,
      },
      StateUpdate {
        operation: Operation::RunBuild,
        start_ts: 200,
        success: true,
      },
    ];

    assert_eq!(build_state(&updates), BuildState::Ok);
  }
}
```

Add `pub(crate) mod latest_states;` to `bin/core/src/helpers/mod.rs`; resource
sibling modules call these crate-visible batch helpers.

- [ ] **Step 2: Run the reducer test to verify RED**

Run: `rtk cargo test -p komodo_core helpers::latest_states::tests::cancel_after_latest_build_uses_previous_build -- --exact`

Expected: FAIL because `StateUpdate` and `build_state` are undefined.

- [ ] **Step 3: Implement the aggregation rows and Build reducer**

Place this code above the test:

```rust
use std::{collections::HashMap, future::Future};

use anyhow::Context;
use database::{
  bson::{Bson, Document, doc, from_bson},
  mungos::mongodb::{Collection, options::AggregateOptions},
};
use futures_util::TryStreamExt;
use komodo_client::entities::{
  Operation,
  action::ActionState,
  build::BuildState,
  procedure::ProcedureState,
  repo::RepoState,
};
use serde::Deserialize;

use crate::state::db_client;

#[derive(Clone, Debug, Deserialize)]
pub struct StateUpdate {
  pub operation: Operation,
  pub start_ts: i64,
  pub success: bool,
}

#[derive(Debug, Deserialize)]
struct ResourceStateRow {
  target_id: String,
  updates: Option<Vec<StateUpdate>>,
}

fn state_lookup(
  target_type: &str,
  operation: &str,
  limit: i64,
  output_field: &str,
) -> Document {
  doc! {
    "$lookup": {
      "from": "Update",
      "let": { "target_id": "$target_id" },
      "pipeline": [
        {
          "$match": {
            "$expr": {
              "$and": [
                { "$eq": ["$target.type", target_type] },
                { "$eq": ["$target.id", "$$target_id"] },
                { "$eq": ["$operation", operation] },
              ],
            },
          },
        },
        { "$sort": { "start_ts": -1 } },
        { "$limit": limit },
        {
          "$project": {
            "_id": 0,
            "operation": 1,
            "start_ts": 1,
            "success": 1,
          },
        },
      ],
      "as": output_field,
    },
  }
}

fn state_pipeline(
  target_type: &str,
  operations: &[(&str, i64)],
) -> Vec<Document> {
  let mut pipeline = vec![doc! {
    "$project": { "target_id": { "$toString": "$_id" } },
  }];
  let mut arrays = Vec::with_capacity(operations.len());
  for (index, (operation, limit)) in operations.iter().enumerate() {
    let field = format!("state_{index}");
    pipeline.push(state_lookup(
      target_type,
      operation,
      *limit,
      &field,
    ));
    arrays.push(Bson::String(format!("${field}")));
  }
  pipeline.push(doc! {
    "$project": {
      "_id": 0,
      "target_id": 1,
      "updates": { "$concatArrays": arrays },
    },
  });
  pipeline
}

async fn aggregate_resource_updates<T: Send + Sync>(
  collection: &Collection<T>,
  target_type: &str,
  operations: &[(&str, i64)],
) -> anyhow::Result<Vec<ResourceStateRow>> {
  let mut cursor = collection
    .aggregate(state_pipeline(target_type, operations))
    .with_options(
      AggregateOptions::builder()
        .comment(Bson::String(format!("p1:state:{target_type}")))
        .batch_size(2_000)
        .build(),
    )
    .await?;
  let mut rows = Vec::new();
  while let Some(document) = cursor.try_next().await? {
    let target_id = document
      .get_str("target_id")
      .context("latest-state row has no target_id")?
      .to_string();
    let updates = document
      .get_array("updates")
      .ok()
      .and_then(|updates| {
        updates
          .iter()
          .cloned()
          .map(from_bson::<StateUpdate>)
          .collect::<Result<Vec<_>, _>>()
          .ok()
      });
    rows.push(ResourceStateRow { target_id, updates });
  }
  Ok(rows)
}

async fn aggregate_resource_ids<T: Send + Sync>(
  collection: &Collection<T>,
  target_type: &str,
) -> anyhow::Result<Vec<String>> {
  let mut cursor = collection
    .aggregate(vec![doc! {
      "$project": {
        "_id": 0,
        "target_id": { "$toString": "$_id" },
      },
    }])
    .with_options(
      AggregateOptions::builder()
        .comment(Bson::String(format!(
          "p1:state-fallback:{target_type}"
        )))
        .batch_size(2_000)
        .build(),
    )
    .await?;
  let mut ids = Vec::new();
  while let Some(document) = cursor.try_next().await? {
    ids.push(
      document
        .get_str("target_id")
        .context("resource-id fallback row has no target_id")?
        .to_string(),
    );
  }
  Ok(ids)
}

async fn states_or_unknown_with<
  S,
  LoadStates,
  LoadStatesFuture,
  LoadIds,
  LoadIdsFuture,
>(
  load_states: LoadStates,
  load_ids: LoadIds,
  unknown: S,
) -> anyhow::Result<HashMap<String, S>>
where
  S: Clone,
  LoadStates: FnOnce() -> LoadStatesFuture,
  LoadStatesFuture:
    Future<Output = anyhow::Result<HashMap<String, S>>>,
  LoadIds: FnOnce() -> LoadIdsFuture,
  LoadIdsFuture: Future<Output = anyhow::Result<Vec<String>>>,
{
  match load_states().await {
    Ok(states) => Ok(states),
    Err(error) => {
      tracing::warn!(
        %error,
        "latest-state aggregate failed; using Unknown fallback"
      );
      Ok(load_ids()
        .await?
        .into_iter()
        .map(|id| (id, unknown.clone()))
        .collect())
    }
  }
}

pub fn build_state(updates: &[StateUpdate]) -> BuildState {
  let mut builds = updates
    .iter()
    .filter(|update| update.operation == Operation::RunBuild)
    .collect::<Vec<_>>();
  builds.sort_by_key(|update| std::cmp::Reverse(update.start_ts));
  let cancel = updates
    .iter()
    .filter(|update| update.operation == Operation::CancelBuild)
    .max_by_key(|update| update.start_ts);
  let selected = match (builds.first(), cancel) {
    (Some(latest), Some(cancel))
      if cancel.start_ts > latest.start_ts =>
    {
      builds.get(1).copied()
    }
    (latest, _) => latest.copied(),
  };
  match selected {
    Some(update) if update.success => BuildState::Ok,
    Some(_) => BuildState::Failed,
    None => BuildState::Ok,
  }
}

fn reduce_build_rows(
  rows: Vec<ResourceStateRow>,
) -> HashMap<String, BuildState> {
  rows
    .into_iter()
    .map(|row| {
      let state = row
        .updates
        .as_deref()
        .map(build_state)
        .unwrap_or(BuildState::Unknown);
      (row.target_id, state)
    })
    .collect()
}

pub async fn build_states() -> anyhow::Result<HashMap<String, BuildState>> {
  states_or_unknown_with(
    || async {
      Ok(reduce_build_rows(
        aggregate_resource_updates(
          &db_client().builds,
          "Build",
          &[("RunBuild", 2), ("CancelBuild", 1)],
        )
        .await?,
      ))
    },
    || aggregate_resource_ids(&db_client().builds, "Build"),
    BuildState::Unknown,
  )
  .await
}
```

The resource collection is the outer side of the aggregation, so never-run
resources still produce a row. Each correlated lookup uses the compound Update
index and limits inside the lookup: Build reads at most two `RunBuild` rows and
one `CancelBuild` row per resource. Work therefore scales with current
resources, not the unbounded Update history, while remaining one Mongo command.

Do not let one malformed `updates` element abort every row. Decode
`target_id` first (it is generated from the outer resource `_id`), then decode
the update array. Represent a decode failure as `updates: None`; each
type-specific reducer maps `None` to that enum's `Unknown` variant while valid
rows retain the existing success/failed/never-run rules.

- [ ] **Step 4: Run the reducer test to verify GREEN**

Run: `rtk cargo test -p komodo_core helpers::latest_states::tests::cancel_after_latest_build_uses_previous_build -- --exact`

Expected: PASS.

- [ ] **Step 5: Add bounded per-operation lookups and reducers**

First add this test beside the Build reducer test:

```rust
#[test]
fn latest_operation_wins_across_repo_operation_families() {
  let updates = vec![
    StateUpdate {
      operation: Operation::CloneRepo,
      start_ts: 100,
      success: true,
    },
    StateUpdate {
      operation: Operation::PullRepo,
      start_ts: 200,
      success: false,
    },
  ];
  assert_eq!(latest_success(&updates), Some(false));
}
```

Replace the test module's `super` import with the complete set now used by its
tests:

```rust
use super::{
  ResourceStateRow, StateUpdate, build_state, latest_success,
  reduce_build_rows, states_or_unknown_with,
};
```

Then run:

```bash
rtk cargo test -p komodo_core helpers::latest_states::tests::latest_operation_wins_across_repo_operation_families -- --exact
```

Expected: FAIL because `latest_success` is undefined.

Append to `latest_states.rs`:

```rust
fn latest_success(updates: &[StateUpdate]) -> Option<bool> {
  updates
    .iter()
    .max_by_key(|update| update.start_ts)
    .map(|update| update.success)
}

fn reduce_latest_rows<S: Clone>(
  rows: Vec<ResourceStateRow>,
  unknown: S,
  ok: S,
  failed: S,
) -> HashMap<String, S> {
  rows
    .into_iter()
    .map(|row| {
      let state = match row.updates.as_deref().map(latest_success) {
        None => unknown.clone(),
        Some(Some(true)) | Some(None) => ok.clone(),
        Some(Some(false)) => failed.clone(),
      };
      (row.target_id, state)
    })
    .collect()
}

pub async fn repo_states() -> anyhow::Result<HashMap<String, RepoState>> {
  states_or_unknown_with(
    || async {
      Ok(reduce_latest_rows(
        aggregate_resource_updates(
          &db_client().repos,
          "Repo",
          &[("CloneRepo", 1), ("PullRepo", 1), ("BuildRepo", 1)],
        )
        .await?,
        RepoState::Unknown,
        RepoState::Ok,
        RepoState::Failed,
      ))
    },
    || aggregate_resource_ids(&db_client().repos, "Repo"),
    RepoState::Unknown,
  )
  .await
}

pub async fn procedure_states(
) -> anyhow::Result<HashMap<String, ProcedureState>> {
  states_or_unknown_with(
    || async {
      Ok(reduce_latest_rows(
        aggregate_resource_updates(
          &db_client().procedures,
          "Procedure",
          &[("RunProcedure", 1)],
        )
        .await?,
        ProcedureState::Unknown,
        ProcedureState::Ok,
        ProcedureState::Failed,
      ))
    },
    || aggregate_resource_ids(
      &db_client().procedures,
      "Procedure",
    ),
    ProcedureState::Unknown,
  )
  .await
}

pub async fn action_states(
) -> anyhow::Result<HashMap<String, ActionState>> {
  states_or_unknown_with(
    || async {
      Ok(reduce_latest_rows(
        aggregate_resource_updates(
          &db_client().actions,
          "Action",
          &[("RunAction", 1)],
        )
        .await?,
        ActionState::Unknown,
        ActionState::Ok,
        ActionState::Failed,
      ))
    },
    || aggregate_resource_ids(&db_client().actions, "Action"),
    ActionState::Unknown,
  )
  .await
}
```

Add two failure-path tests before routing the refresh loops:

```rust
#[test]
fn malformed_row_is_unknown_without_poisoning_other_resources() {
  let rows = vec![
    ResourceStateRow {
      target_id: "good".into(),
      updates: Some(vec![]),
    },
    ResourceStateRow {
      target_id: "bad".into(),
      updates: None,
    },
  ];
  let states = reduce_build_rows(rows);
  assert_eq!(states["good"], BuildState::Ok);
  assert_eq!(states["bad"], BuildState::Unknown);
}

#[tokio::test]
async fn aggregate_failure_falls_back_to_unknown_for_all_ids() {
  let states = states_or_unknown_with(
    || async { anyhow::bail!("update lookup failed") },
    || async { Ok(vec!["one".into(), "two".into()]) },
    BuildState::Unknown,
  )
  .await
  .unwrap();
  assert_eq!(states["one"], BuildState::Unknown);
  assert_eq!(states["two"], BuildState::Unknown);
}
```

The Step 3 `states_or_unknown_with` helper is the single sequencing path used
by all four production `*_states` functions above. Each first runs its single
aggregate. If that command fails, it logs the error and issues one
projection-only aggregation against the outer resource collection, returning
`Unknown` for every current ID. Failure of both the state aggregate and the ID
fallback remains an error. This preserves the existing error semantics; the
extra resource-ID command exists only on the failure path and is excluded from
the healthy one-command budget.

- [ ] **Step 6: Replace the Build refresh loop**

Replace `refresh_build_state_cache` in `bin/core/src/resource/build.rs` with:

```rust
pub async fn refresh_build_state_cache() {
  let _ = async {
    let states = crate::helpers::latest_states::build_states()
      .await
    .context("failed to load batched build states")?;
    let cache = build_state_cache();
    for (id, state) in states {
      cache.insert(id, state).await;
    }
    anyhow::Ok(())
  }
  .await
  .inspect_err(|e| {
    error!("failed to refresh build state cache | {e:#}")
  });
}
```

Delete `get_build_state_from_db` and `latest_2_build_updates`. Remove now-unused `FindOptions` and `get_latest_update` imports.

- [ ] **Step 7: Replace Repo, Procedure, and Action refresh bodies**

Replace the three refresh bodies with these exact type-specific loops:

```rust
// repo.rs
let states = crate::helpers::latest_states::repo_states().await?;
for (id, state) in states {
  repo_state_cache().insert(id, state).await;
}

// procedure.rs
let states = crate::helpers::latest_states::procedure_states().await?;
for (id, state) in states {
  procedure_state_cache().insert(id, state).await;
}

// action.rs
let states = crate::helpers::latest_states::action_states().await?;
for (id, state) in states {
  action_state_cache().insert(id, state).await;
}
```

Retain each existing outer error log. Delete the four per-resource `get_*_state_from_db` helpers and their unused `FindOneOptions` imports.

- [ ] **Step 8: Add an executable explain gate for all four pipelines**

Create `scripts/performance/explain-latest-state-aggregations.js`:

```javascript
function stateLookup(targetType, operation, limit, outputField) {
  return {
    $lookup: {
      from: "Update",
      let: { target_id: "$target_id" },
      pipeline: [
        {
          $match: {
            $expr: {
              $and: [
                { $eq: ["$target.type", targetType] },
                { $eq: ["$target.id", "$$target_id"] },
                { $eq: ["$operation", operation] },
              ],
            },
          },
        },
        { $sort: { start_ts: -1 } },
        { $limit: limit },
        { $project: { _id: 0, operation: 1, start_ts: 1, success: 1 } },
      ],
      as: outputField,
    },
  };
}

function statePipeline(targetType, operations) {
  const pipeline = [
    { $project: { target_id: { $toString: "$_id" } } },
  ];
  const arrays = [];
  for (const [index, [operation, limit]] of operations.entries()) {
    const outputField = `state_${index}`;
    pipeline.push(
      stateLookup(targetType, operation, limit, outputField),
    );
    arrays.push(`$${outputField}`);
  }
  pipeline.push({
    $project: {
      _id: 0,
      target_id: 1,
      updates: { $concatArrays: arrays },
    },
  });
  return pipeline;
}

const cases = [
  {
    collection: "Build",
    operations: [["RunBuild", 2], ["CancelBuild", 1]],
    maxUpdateDocsPerResource: 3,
  },
  {
    collection: "Repo",
    operations: [["CloneRepo", 1], ["PullRepo", 1], ["BuildRepo", 1]],
    maxUpdateDocsPerResource: 3,
  },
  {
    collection: "Procedure",
    operations: [["RunProcedure", 1]],
    maxUpdateDocsPerResource: 1,
  },
  {
    collection: "Action",
    operations: [["RunAction", 1]],
    maxUpdateDocsPerResource: 1,
  },
];

for (const testCase of cases) {
  const resources = db.getCollection(testCase.collection).countDocuments();
  if (resources === 0) {
    throw new Error(`${testCase.collection} fixture is empty`);
  }
  const explanation = db
    .getCollection(testCase.collection)
    .explain("executionStats")
    .aggregate(
      statePipeline(testCase.collection, testCase.operations),
    );
  const lookups = (explanation.stages || []).filter(
    (stage) => stage.$lookup,
  );
  if (lookups.length !== testCase.operations.length) {
    throw new Error(
      `${testCase.collection} explain did not expose every lookup stage`,
    );
  }
  const updateDocsExamined = lookups.reduce(
    (total, stage) => total + Number(stage.totalDocsExamined || 0),
    0,
  );
  const collectionScans = lookups.reduce(
    (total, stage) => total + Number(stage.collectionScans || 0),
    0,
  );
  const indexesUsed = lookups.map((stage) => stage.indexesUsed || []);
  const result = {
    collection: testCase.collection,
    resources,
    lookupStages: lookups.length,
    updateDocsExamined,
    updateDocsExaminedPerResource: updateDocsExamined / resources,
    collectionScans,
    indexesUsed,
  };
  print(EJSON.stringify(result));
  if (collectionScans !== 0 || indexesUsed.some((names) => names.length === 0)) {
    throw new Error(`${testCase.collection} lookup is not index-backed`);
  }
  if (
    updateDocsExamined >
    resources * testCase.maxUpdateDocsPerResource
  ) {
    throw new Error(
      `${testCase.collection} exceeded bounded Update-row reads`,
    );
  }
}
```

This executes the same outer-resource/correlated-Update shape as the Rust
pipeline. A differently named but equivalent production index is valid; every
lookup must nevertheless report an index and zero collection scans.

- [ ] **Step 9: Run focused and full tests**

Run:

```bash
rtk cargo test -p komodo_core helpers::latest_states
rtk cargo test -p komodo_core
```

Expected: all tests PASS.

- [ ] **Step 10: Measure the state-refresh query budget**

Add these environment-gated ignored tests to the existing
`helpers::latest_states::tests` module. They call the real aggregation helpers;
the shared initializer also makes running all four ignored tests in one process
safe.

```rust
async fn init_profile_db() {
  static INIT: tokio::sync::OnceCell<()> =
    tokio::sync::OnceCell::const_new();
  INIT.get_or_init(|| async {
    crate::state::init_db_client().await;
  })
  .await;
}

fn expected_profile_size() -> usize {
  std::env::var("P1_FIXTURE_SIZE")
    .expect("P1_FIXTURE_SIZE is required")
    .parse()
    .expect("P1_FIXTURE_SIZE must be an integer")
}

#[tokio::test]
#[ignore = "requires a manifest-validated staging MongoDB"]
async fn profile_build_states_once() {
  init_profile_db().await;
  assert_eq!(
    super::build_states().await.unwrap().len(),
    expected_profile_size(),
  );
}

#[tokio::test]
#[ignore = "requires a manifest-validated staging MongoDB"]
async fn profile_repo_states_once() {
  init_profile_db().await;
  assert_eq!(
    super::repo_states().await.unwrap().len(),
    expected_profile_size(),
  );
}

#[tokio::test]
#[ignore = "requires a manifest-validated staging MongoDB"]
async fn profile_procedure_states_once() {
  init_profile_db().await;
  assert_eq!(
    super::procedure_states().await.unwrap().len(),
    expected_profile_size(),
  );
}

#[tokio::test]
#[ignore = "requires a manifest-validated staging MongoDB"]
async fn profile_action_states_once() {
  init_profile_db().await;
  assert_eq!(
    super::action_states().await.unwrap().len(),
    expected_profile_size(),
  );
}
```

Immediately after the two Task 4 monitoring calls and before the
`artifact_lines` check, append these four exact probes to
`profile-core-background-cycles.sh`:

```sh
run_probe \
  'helpers::latest_states::tests::profile_build_states_once' \
  'p1:state:Build' 1 4 16777216
run_probe \
  'helpers::latest_states::tests::profile_repo_states_once' \
  'p1:state:Repo' 1 4 16777216
run_probe \
  'helpers::latest_states::tests::profile_procedure_states_once' \
  'p1:state:Procedure' 1 4 16777216
run_probe \
  'helpers::latest_states::tests::profile_action_states_once' \
  'p1:state:Action' 1 4 16777216
```

Each state probe expects one logical aggregate, at most four getMore
operations, and at most 16 MiB of profiler response bytes.

Run all inventory and state probes on every fixture:

```bash
rtk env FIXTURE_SIZE=1 FIXTURE_MANIFEST="$FIXTURE_MANIFEST_1" MONGODB_URI="$MONGODB_URI_FIXTURE_1" scripts/performance/profile-core-background-cycles.sh
rtk env FIXTURE_SIZE=100 FIXTURE_MANIFEST="$FIXTURE_MANIFEST_100" MONGODB_URI="$MONGODB_URI_FIXTURE_100" scripts/performance/profile-core-background-cycles.sh
rtk env FIXTURE_SIZE=1000 FIXTURE_MANIFEST="$FIXTURE_MANIFEST_1000" MONGODB_URI="$MONGODB_URI_FIXTURE_1000" scripts/performance/profile-core-background-cycles.sh
rtk mongosh "$MONGODB_URI_FIXTURE_1000" --quiet scripts/performance/explain-latest-state-aggregations.js
```

Expected: exactly one logical aggregation start per type at every size;
getMore and response work stay under their separate bounds. Explain output
shows every correlated Update lookup is index-backed with no collection scan;
`totalDocsExamined` in Update is at most three per resource for Build/Repo and
one for Procedure/Action. State values for
successful, failed, cancelled, and never-run resources match the pre-change
results.

- [ ] **Step 11: Commit batched latest-state reads**

```bash
rtk git add bin/core/src/helpers/latest_states.rs bin/core/src/helpers/mod.rs bin/core/src/resource/build.rs bin/core/src/resource/repo.rs bin/core/src/resource/procedure.rs bin/core/src/resource/action.rs scripts/performance/explain-latest-state-aggregations.js scripts/performance/profile-core-background-cycles.sh scripts/performance/capture-core-background-profile.js
rtk git commit -m "perf: batch latest resource state reads"
```

Expected: one focused state-refresh commit.

### Task 6: Close checkpoint 2 with fixed-query regression evidence

**Files:**
- Modify: `docs/performance/core-data-path-budgets.md`

- [ ] **Step 1: Add the measured checkpoint table**

Append the exact command-count table below, replacing no values because the numbers are the release gates:

```markdown
## Checkpoint 2 logical-query and cursor-work gates

| Cycle | Logical 1/100/1,000 | Max getMore | Max response bytes |
|---|---:|---:|---:|
| Server inventory | 4 / 4 / 4 | 12 | 67,108,864 |
| Swarm inventory | 3 / 3 / 3 | 8 | 67,108,864 |
| Build state | 1 / 1 / 1 | 4 | 16,777,216 |
| Repo state | 1 / 1 / 1 | 4 | 16,777,216 |
| Procedure state | 1 / 1 / 1 | 4 | 16,777,216 |
| Action state | 1 / 1 / 1 | 4 | 16,777,216 |

Logical means initial `find`/`aggregate`, not cursor continuation. Any larger
logical, getMore, or response-byte number fails. `docsExamined`,
`keysExamined`, and `millis` remain in every raw artifact attached to the PR.
```

- [ ] **Step 2: Run checkpoint verification**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo test -p database
rtk cargo test -p komodo_core
```

Expected: all commands exit 0.

- [ ] **Step 3: Commit the fixed-query evidence gate**

```bash
rtk git add docs/performance/core-data-path-budgets.md
rtk git commit -m "docs: freeze core refresh query gates"
```

Expected: checkpoint 2 is independently deployable and does not depend on permission caching.

### Task 7: Introduce atomic per-type resource snapshots and dirty generations

**Files:**
- Rewrite: `bin/core/src/helpers/all_resources.rs:1-78`
- Test: `bin/core/src/helpers/all_resources.rs`

- [ ] **Step 1: Write the failing dirty-generation tests**

Append these tests to `bin/core/src/helpers/all_resources.rs` before changing its implementation:

```rust
#[cfg(test)]
mod tests {
  use std::{collections::HashMap, sync::Arc};

  use komodo_client::entities::server::Server;

  use super::{
    AllResourcesCache, CachedResource, ResourceTypeSnapshot,
  };

  #[test]
  fn stale_repair_neither_publishes_nor_clears_dirty() {
    let snapshot = ResourceTypeSnapshot::<Server>::default();
    let mut initial = HashMap::new();
    let mut server = Server::default();
    server.id = "server-1".into();
    initial.insert(server.id.clone(), server);
    assert!(snapshot.publish_repair(Arc::new(initial), 0));

    let first = snapshot.mark_dirty();
    let _second = snapshot.mark_dirty();
    assert!(!snapshot.publish_repair(
      Arc::new(HashMap::new()),
      first,
    ));
    assert!(snapshot.is_dirty());
    assert!(snapshot.load().contains_key("server-1"));
  }

  #[tokio::test]
  async fn dirty_read_repairs_and_marks_clean() {
    let snapshot = ResourceTypeSnapshot::<Server>::default();
    snapshot.mark_dirty();
    let mut authoritative = HashMap::new();
    let mut server = Server::default();
    server.id = "server-1".into();
    authoritative.insert(server.id.clone(), server);

    let read = snapshot
      .read_with(|| {
        let authoritative = authoritative.clone();
        async move { Ok(Arc::new(authoritative)) }
      })
      .await
      .unwrap();

    assert!(read.contains_key("server-1"));
    assert!(!snapshot.is_dirty());
  }

  #[test]
  fn parallel_upserts_do_not_lose_a_resource() {
    let cache = Arc::new(AllResourcesCache::default());
    let handles = ["server-1", "server-2"].map(|id| {
      let cache = cache.clone();
      std::thread::spawn(move || {
        let mut server = Server::default();
        server.id = id.to_string();
        cache.upsert::<Server>(server);
      })
    });
    for handle in handles {
      handle.join().unwrap();
    }
    let servers =
      <Server as CachedResource>::snapshot(&cache).load();
    assert!(servers.contains_key("server-1"));
    assert!(servers.contains_key("server-2"));
  }

  #[test]
  fn upsert_does_not_clear_preexisting_dirty_state() {
    let cache = AllResourcesCache::default();
    cache.mark_dirty::<Server>();
    let mut server = Server::default();
    server.id = "server-1".into();
    cache.upsert::<Server>(server);
    assert!(
      <Server as CachedResource>::snapshot(&cache).is_dirty()
    );
  }
}
```

- [ ] **Step 2: Run the tests to verify RED**

Run: `rtk cargo test -p komodo_core helpers::all_resources::tests -- --nocapture`

Expected: FAIL because `ResourceTypeSnapshot` does not exist.

- [ ] **Step 3: Replace the monolithic value with per-type snapshots**

Rewrite `bin/core/src/helpers/all_resources.rs` around these complete types and methods:

```rust
use std::{
  collections::HashMap,
  future::Future,
  sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicU64, Ordering},
  },
};

use arc_swap::ArcSwap;
use komodo_client::entities::{
  action::Action, alerter::Alerter, build::Build, builder::Builder,
  deployment::Deployment, procedure::Procedure, repo::Repo,
  resource::Resource,
  server::Server, stack::Stack, swarm::Swarm, sync::ResourceSync,
};

use crate::resource::{IdResourceMap, KomodoResource};
use tokio::sync::Mutex as AsyncMutex;

#[derive(Clone, Debug, Default)]
pub struct AllResourcesById {
  pub swarms: Arc<HashMap<String, Swarm>>,
  pub servers: Arc<HashMap<String, Server>>,
  pub deployments: Arc<HashMap<String, Deployment>>,
  pub stacks: Arc<HashMap<String, Stack>>,
  pub builds: Arc<HashMap<String, Build>>,
  pub repos: Arc<HashMap<String, Repo>>,
  pub procedures: Arc<HashMap<String, Procedure>>,
  pub actions: Arc<HashMap<String, Action>>,
  pub builders: Arc<HashMap<String, Builder>>,
  pub alerters: Arc<HashMap<String, Alerter>>,
  pub syncs: Arc<HashMap<String, ResourceSync>>,
}

pub struct ResourceTypeSnapshot<T: KomodoResource> {
  resources: ArcSwap<IdResourceMap<T>>,
  dirty_generation: AtomicU64,
  clean_generation: AtomicU64,
  publish_lock: StdMutex<()>,
  refresh_lock: AsyncMutex<()>,
}

impl<T: KomodoResource> Default for ResourceTypeSnapshot<T> {
  fn default() -> Self {
    Self {
      resources: ArcSwap::from_pointee(HashMap::new()),
      dirty_generation: AtomicU64::new(0),
      clean_generation: AtomicU64::new(0),
      publish_lock: StdMutex::new(()),
      refresh_lock: AsyncMutex::new(()),
    }
  }
}

impl<T: KomodoResource> ResourceTypeSnapshot<T> {
  pub fn load(&self) -> Arc<IdResourceMap<T>> {
    self.resources.load_full()
  }

  pub fn mark_dirty(&self) -> u64 {
    let _publish = self
      .publish_lock
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner());
    self.dirty_generation.fetch_add(1, Ordering::AcqRel) + 1
  }

  pub fn is_dirty(&self) -> bool {
    self.clean_generation.load(Ordering::Acquire)
      < self.dirty_generation.load(Ordering::Acquire)
  }

  pub fn publish_repair(
    &self,
    resources: Arc<IdResourceMap<T>>,
    observed_generation: u64,
  ) -> bool {
    let _publish = self
      .publish_lock
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner());
    if self.dirty_generation.load(Ordering::Acquire)
      != observed_generation
    {
      return false;
    }
    self.resources.store(resources);
    self.clean_generation
      .store(observed_generation, Ordering::Release);
    true
  }

  async fn refresh_with<F, Fut>(
    &self,
    mut load: F,
  ) -> anyhow::Result<bool>
  where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<Arc<IdResourceMap<T>>>>,
  {
    let _refresh = self.refresh_lock.lock().await;
    let observed =
      self.dirty_generation.load(Ordering::Acquire);
    let resources = load().await?;
    Ok(self.publish_repair(resources, observed))
  }

  async fn read_with<F, Fut>(
    &self,
    mut load: F,
  ) -> anyhow::Result<Arc<IdResourceMap<T>>>
  where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<Arc<IdResourceMap<T>>>>,
  {
    if !self.is_dirty() {
      return Ok(self.load());
    }
    let _refresh = self.refresh_lock.lock().await;
    if !self.is_dirty() {
      return Ok(self.load());
    }
    for _ in 0..3 {
      let observed =
        self.dirty_generation.load(Ordering::Acquire);
      let resources = load().await?;
      if self.publish_repair(resources, observed) {
        return Ok(self.load());
      }
    }
    anyhow::bail!("resource cache generation remained unstable")
  }
}

#[derive(Default)]
pub struct AllResourcesCache {
  swarms: ResourceTypeSnapshot<Swarm>,
  servers: ResourceTypeSnapshot<Server>,
  deployments: ResourceTypeSnapshot<Deployment>,
  stacks: ResourceTypeSnapshot<Stack>,
  builds: ResourceTypeSnapshot<Build>,
  repos: ResourceTypeSnapshot<Repo>,
  procedures: ResourceTypeSnapshot<Procedure>,
  actions: ResourceTypeSnapshot<Action>,
  builders: ResourceTypeSnapshot<Builder>,
  alerters: ResourceTypeSnapshot<Alerter>,
  syncs: ResourceTypeSnapshot<ResourceSync>,
}

pub trait CachedResource: KomodoResource + Sized {
  fn snapshot(
    cache: &AllResourcesCache,
  ) -> &ResourceTypeSnapshot<Self>;
  fn set_map(
    all: &mut AllResourcesById,
    map: Arc<IdResourceMap<Self>>,
  );
}

macro_rules! impl_cached_resource {
  ($ty:ty, $field:ident) => {
    impl CachedResource for $ty {
      fn snapshot(
        cache: &AllResourcesCache,
      ) -> &ResourceTypeSnapshot<Self> {
        &cache.$field
      }

      fn set_map(
        all: &mut AllResourcesById,
        map: Arc<IdResourceMap<Self>>,
      ) {
        all.$field = map;
      }
    }
  };
}

impl_cached_resource!(Swarm, swarms);
impl_cached_resource!(Server, servers);
impl_cached_resource!(Deployment, deployments);
impl_cached_resource!(Stack, stacks);
impl_cached_resource!(Build, builds);
impl_cached_resource!(Repo, repos);
impl_cached_resource!(Procedure, procedures);
impl_cached_resource!(Action, actions);
impl_cached_resource!(Builder, builders);
impl_cached_resource!(Alerter, alerters);
impl_cached_resource!(ResourceSync, syncs);
```

- [ ] **Step 4: Add dirty reads, nonblocking refresh, and atomic per-type updates**

Append to `all_resources.rs`:

```rust
impl AllResourcesCache {
  async fn load_type<T: CachedResource>(
  ) -> anyhow::Result<Arc<IdResourceMap<T>>> {
    Ok(Arc::new(
      crate::resource::get_id_to_resource_map::<T>(
        &HashMap::new(),
        &[],
      )
      .await?,
    ))
  }

  pub async fn read_type<T: CachedResource>(
    &self,
  ) -> anyhow::Result<Arc<IdResourceMap<T>>> {
    T::snapshot(self)
      .read_with(|| Self::load_type::<T>())
      .await
  }

  pub async fn read(&self) -> anyhow::Result<AllResourcesById> {
    let (
      swarms,
      servers,
      deployments,
      stacks,
      builds,
      repos,
      procedures,
      actions,
      builders,
      alerters,
      syncs,
    ) = tokio::try_join!(
      self.read_type::<Swarm>(),
      self.read_type::<Server>(),
      self.read_type::<Deployment>(),
      self.read_type::<Stack>(),
      self.read_type::<Build>(),
      self.read_type::<Repo>(),
      self.read_type::<Procedure>(),
      self.read_type::<Action>(),
      self.read_type::<Builder>(),
      self.read_type::<Alerter>(),
      self.read_type::<ResourceSync>(),
    )?;
    Ok(AllResourcesById {
      swarms,
      servers,
      deployments,
      stacks,
      builds,
      repos,
      procedures,
      actions,
      builders,
      alerters,
      syncs,
    })
  }

  pub fn mark_dirty<T: CachedResource>(&self) -> u64 {
    T::snapshot(self).mark_dirty()
  }

  pub fn upsert<T: CachedResource>(
    &self,
    resource: Resource<T::Config, T::Info>,
  ) {
    let snapshot = T::snapshot(self);
    let _publish = snapshot
      .publish_lock
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_dirty = snapshot
      .dirty_generation
      .fetch_add(1, Ordering::AcqRel);
    let observed = previous_dirty + 1;
    let mut resources = (*snapshot.load()).clone();
    resources.insert(resource.id.clone(), resource);
    snapshot.resources.store(Arc::new(resources));
    if snapshot.clean_generation.load(Ordering::Acquire)
      == previous_dirty
    {
      let _ = snapshot.clean_generation.compare_exchange(
        previous_dirty,
        observed,
        Ordering::AcqRel,
        Ordering::Acquire,
      );
    }
  }

  pub fn remove<T: CachedResource>(&self, id: &str) {
    let snapshot = T::snapshot(self);
    let _publish = snapshot
      .publish_lock
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_dirty = snapshot
      .dirty_generation
      .fetch_add(1, Ordering::AcqRel);
    let observed = previous_dirty + 1;
    let mut resources = (*snapshot.load()).clone();
    resources.remove(id);
    snapshot.resources.store(Arc::new(resources));
    if snapshot.clean_generation.load(Ordering::Acquire)
      == previous_dirty
    {
      let _ = snapshot.clean_generation.compare_exchange(
        previous_dirty,
        observed,
        Ordering::AcqRel,
        Ordering::Acquire,
      );
    }
  }

  pub async fn repair_type<T: CachedResource>(
    &self,
  ) -> anyhow::Result<()> {
    self.read_type::<T>().await.map(|_| ())
  }

  pub async fn repair_dirty(&self) -> anyhow::Result<()> {
    tokio::try_join!(
      self.repair_type::<Swarm>(),
      self.repair_type::<Server>(),
      self.repair_type::<Deployment>(),
      self.repair_type::<Stack>(),
      self.repair_type::<Build>(),
      self.repair_type::<Repo>(),
      self.repair_type::<Procedure>(),
      self.repair_type::<Action>(),
      self.repair_type::<Builder>(),
      self.repair_type::<Alerter>(),
      self.repair_type::<ResourceSync>(),
    )?;
    Ok(())
  }

  pub async fn refresh_type<T: CachedResource>(
    &self,
  ) -> anyhow::Result<bool> {
    T::snapshot(self)
      .refresh_with(|| Self::load_type::<T>())
      .await
  }

  pub async fn refresh_all(&self) -> anyhow::Result<()> {
    tokio::try_join!(
      self.refresh_type::<Swarm>(),
      self.refresh_type::<Server>(),
      self.refresh_type::<Deployment>(),
      self.refresh_type::<Stack>(),
      self.refresh_type::<Build>(),
      self.refresh_type::<Repo>(),
      self.refresh_type::<Procedure>(),
      self.refresh_type::<Action>(),
      self.refresh_type::<Builder>(),
      self.refresh_type::<Alerter>(),
      self.refresh_type::<ResourceSync>(),
    )?;
    Ok(())
  }
}

impl AllResourcesById {
  pub async fn load() -> anyhow::Result<Self> {
    let cache = AllResourcesCache::default();
    cache.refresh_all().await?;
    cache.read().await
  }
}
```

The async `refresh_lock` is per resource type. One dirty Repo generation and
1,000 concurrent Build list-item conversions therefore cause one authoritative
Repo load; all other readers await it and consume the published snapshot. If a
mutation advances the generation during the load, the owner retries at most
three times and then fails rather than returning stale data.

The same per-type lock also serializes the fifteen-second convergence load
with dirty read-through, but a clean reader never acquires it: it keeps the
current `Arc` while convergence runs and observes the replacement on its next
read after the atomic swap. `refresh_with` samples the generation, performs
the database load under only the async refresh lock, and takes the short
`publish_lock` after the await. It publishes only when that generation is
still current. `mark_dirty`, `upsert`, `remove`, and `publish_repair` all
linearize under `publish_lock`, so a concurrent mutation cannot be overwritten
by an older convergence result; the standard mutex is never held across I/O.

The `compare_exchange` is intentional: an incremental publish may mark itself
clean only when the generation was clean immediately before that publish. A
concurrent `mark_dirty` either changes `previous_dirty` before the publish or
leaves `dirty_generation > clean_generation` afterward; it can never be
cleared accidentally by `upsert` or `remove`.

- [ ] **Step 5: Add singleflight and background-refresh race regressions**

Add `dirty_reads_are_singleflight`, a Tokio test that marks a
`ResourceTypeSnapshot<Repo>` dirty, launches
1,000 `read_with` calls behind a barrier, and increments an `AtomicUsize` in
the injected loader. Every result contains the same Repo and the loader count
is exactly one. Add a second test whose first loader call advances the dirty
generation; it must retry and publish only the second map.

At this step, replace the test module imports with this complete block so the
singleflight and refresh-race tests compile:

```rust
use std::{
  collections::HashMap,
  sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
  },
  time::Duration,
};

use komodo_client::entities::{repo::Repo, server::Server};
use tokio::{
  sync::{Barrier, Notify},
  time::timeout,
};
```

Add these three deterministic background-refresh tests in the same module:

```rust
#[tokio::test]
async fn clean_read_does_not_wait_for_background_refresh() {
  let snapshot =
    Arc::new(ResourceTypeSnapshot::<Server>::default());
  let mut initial_map = HashMap::new();
  let mut initial_server = Server::default();
  initial_server.id = "server-1".into();
  initial_map.insert(initial_server.id.clone(), initial_server);
  let initial = Arc::new(initial_map);
  assert!(snapshot.publish_repair(initial.clone(), 0));

  let refresh_started = Arc::new(Notify::new());
  let finish_refresh = Arc::new(Notify::new());
  let refresh = {
    let snapshot = snapshot.clone();
    let refresh_started = refresh_started.clone();
    let finish_refresh = finish_refresh.clone();
    tokio::spawn(async move {
      snapshot
        .refresh_with(|| {
          let refresh_started = refresh_started.clone();
          let finish_refresh = finish_refresh.clone();
          async move {
            refresh_started.notify_one();
            finish_refresh.notified().await;
            let mut refreshed = HashMap::new();
            let mut server = Server::default();
            server.id = "server-2".into();
            refreshed.insert(server.id.clone(), server);
            Ok(Arc::new(refreshed))
          }
        })
        .await
        .unwrap()
    })
  };

  refresh_started.notified().await;
  let loader_calls = Arc::new(AtomicUsize::new(0));
  let current = timeout(Duration::from_secs(1), snapshot.read_with({
    let loader_calls = loader_calls.clone();
    move || {
      let loader_calls = loader_calls.clone();
      async move {
        loader_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(HashMap::new()))
      }
    }
  }))
  .await
  .expect("clean read blocked on convergence refresh")
  .unwrap();
  assert!(Arc::ptr_eq(&current, &initial));
  assert_eq!(loader_calls.load(Ordering::SeqCst), 0);

  finish_refresh.notify_one();
  assert!(refresh.await.unwrap());
  assert!(snapshot.load().contains_key("server-2"));
}

#[tokio::test]
async fn refresh_does_not_overwrite_concurrent_upsert() {
  let cache = Arc::new(AllResourcesCache::default());
  let mut initial = HashMap::new();
  let mut server = Server::default();
  server.id = "server-1".into();
  initial.insert(server.id.clone(), server);
  assert!(
    <Server as CachedResource>::snapshot(&cache)
      .publish_repair(Arc::new(initial), 0)
  );

  let refresh_started = Arc::new(Notify::new());
  let finish_refresh = Arc::new(Notify::new());
  let refresh = {
    let cache = cache.clone();
    let refresh_started = refresh_started.clone();
    let finish_refresh = finish_refresh.clone();
    tokio::spawn(async move {
      <Server as CachedResource>::snapshot(&cache)
        .refresh_with(|| {
          let refresh_started = refresh_started.clone();
          let finish_refresh = finish_refresh.clone();
          async move {
            refresh_started.notify_one();
            finish_refresh.notified().await;
            Ok(Arc::new(HashMap::new()))
          }
        })
        .await
        .unwrap()
    })
  };

  refresh_started.notified().await;
  let mut concurrent = Server::default();
  concurrent.id = "server-2".into();
  cache.upsert::<Server>(concurrent);
  finish_refresh.notify_one();

  assert!(!refresh.await.unwrap());
  let servers = <Server as CachedResource>::snapshot(&cache).load();
  assert!(servers.contains_key("server-1"));
  assert!(servers.contains_key("server-2"));
}

#[tokio::test]
async fn refresh_does_not_overwrite_concurrent_dirty_mark() {
  let cache = Arc::new(AllResourcesCache::default());
  let mut initial = HashMap::new();
  let mut server = Server::default();
  server.id = "server-1".into();
  initial.insert(server.id.clone(), server);
  assert!(
    <Server as CachedResource>::snapshot(&cache)
      .publish_repair(Arc::new(initial), 0)
  );

  let refresh_started = Arc::new(Notify::new());
  let finish_refresh = Arc::new(Notify::new());
  let refresh = {
    let cache = cache.clone();
    let refresh_started = refresh_started.clone();
    let finish_refresh = finish_refresh.clone();
    tokio::spawn(async move {
      <Server as CachedResource>::snapshot(&cache)
        .refresh_with(|| {
          let refresh_started = refresh_started.clone();
          let finish_refresh = finish_refresh.clone();
          async move {
            refresh_started.notify_one();
            finish_refresh.notified().await;
            Ok(Arc::new(HashMap::new()))
          }
        })
        .await
        .unwrap()
    })
  };

  refresh_started.notified().await;
  cache.mark_dirty::<Server>();
  finish_refresh.notify_one();

  assert!(!refresh.await.unwrap());
  let snapshot = <Server as CachedResource>::snapshot(&cache);
  assert!(snapshot.load().contains_key("server-1"));
  assert!(snapshot.is_dirty());
}
```

The first test holds the per-type refresh lock around an injected slow load and
proves a clean reader immediately receives the exact old `Arc`; it must neither
wait nor invoke its authoritative loader. The other two force a generation
change while that load is in flight and prove the stale result is discarded
instead of replacing an incremental publish or clearing a dirty mark.

- [ ] **Step 6: Run the focused tests to verify GREEN**

Run:

```bash
rtk cargo test -p komodo_core helpers::all_resources::tests -- --nocapture
rtk cargo test -p komodo_core helpers::all_resources::tests::clean_read_does_not_wait_for_background_refresh -- --exact
rtk cargo test -p komodo_core helpers::all_resources::tests::refresh_does_not_overwrite_concurrent_upsert -- --exact
rtk cargo test -p komodo_core helpers::all_resources::tests::refresh_does_not_overwrite_concurrent_dirty_mark -- --exact
rtk cargo check -p komodo_core
```

Expected: all nine tests PASS. They prove a stale repair cannot publish, two
parallel upserts cannot lose either resource, and an incremental update cannot
clear dirty state left by an earlier failed refresh. Two tests prove dirty-read
singleflight and generation retry; three prove nonblocking clean reads and
generation-safe convergence publication. The existing global accessor and
refresh loop remain untouched in this commit; there is no unsafe compatibility
`store` adapter.

- [ ] **Step 7: Commit the per-type cache primitive**

```bash
rtk git add bin/core/src/helpers/all_resources.rs
rtk git commit -m "perf: add per-type resource snapshots"
```

Expected: the commit contains the cache primitive only; mutation call sites still use the compatibility repair path in the next task.

### Task 8: Publish affected resources and add dirty read-through repair

**Files:**
- Modify: `bin/core/src/state.rs:1-4,23-29,211-215`
- Modify: `bin/core/src/resource/refresh.rs:14-40`
- Modify: `bin/core/src/resource/mod.rs:473-552,559-650,687-746,763-881`
- Modify: `bin/core/src/resource/{builder,server}.rs`
- Modify: `bin/core/src/connection/{mod,server}.rs`
- Modify: `bin/core/src/api/write/{build,deployment,repo,stack,sync}.rs`
- Modify: `bin/core/src/api/execute/{build,repo,stack,sync}.rs`
- Modify: `bin/core/src/helpers/procedure.rs:672`
- Modify: `bin/core/src/resource/{build,stack,sync}.rs`
- Modify: `bin/core/src/api/{execute,write}/sync.rs`
- Modify: `bin/core/src/api/read/toml.rs`
- Modify: `bin/core/src/sync/mod.rs`
- Modify: `bin/core/src/sync/{deploy,execute,resources,toml,user_groups,view}.rs`
- Create: `scripts/performance/audit-resource-cache-writers.sh`
- Create: `docs/performance/resource-cache-writer-inventory.md`
- Test: `bin/core/src/resource/refresh.rs`

- [ ] **Step 1: Write the failing repair-race test**

Add to `bin/core/src/resource/refresh.rs`:

```rust
#[cfg(test)]
mod tests {
  use komodo_client::entities::server::Server;

  use crate::helpers::all_resources::AllResourcesCache;

  #[tokio::test]
  async fn upsert_and_remove_publish_only_the_server_type() {
    let cache = AllResourcesCache::default();
    let mut server = Server::default();
    server.id = "server-1".into();
    cache.upsert::<Server>(server);
    assert!(
      cache
        .read_type::<Server>()
        .await
        .unwrap()
        .contains_key("server-1")
    );
    cache.remove::<Server>("server-1");
    assert!(
      !cache
        .read_type::<Server>()
        .await
        .unwrap()
        .contains_key("server-1")
    );
  }
}
```

This test initially fails to compile if `mark_dirty`/`read_type` were not exposed exactly as Task 7 specifies.

- [ ] **Step 2: Switch the global accessor atomically and preserve convergence**

Replace `all_resources_cache` in `bin/core/src/state.rs` with:

```rust
pub fn all_resources_cache() -> &'static AllResourcesCache {
  static ALL_RESOURCES: OnceLock<AllResourcesCache> = OnceLock::new();
  ALL_RESOURCES.get_or_init(Default::default)
}
```

Import `AllResourcesCache` instead of `AllResourcesById` and remove the
now-unused `arc_swap::ArcSwap` import. This is the first step that changes the
global accessor; Task 7 never publishes a monolithic stale map into the new
cache.

Replace `spawn_all_resources_cache_refresh_loop` and
`refresh_all_resources_cache` with:

```rust
pub fn spawn_all_resources_cache_refresh_loop() {
  tokio::spawn(async move {
    if let Err(e) = all_resources_cache().refresh_all().await {
      error!("failed initial all-resources refresh | {e:#}");
    }
    let mut dirty = tokio::time::interval(Duration::from_secs(1));
    let mut convergence =
      tokio::time::interval(Duration::from_secs(15));
    // Tokio intervals tick immediately once; consume those ticks so the
    // explicit initial refresh is not duplicated.
    dirty.tick().await;
    convergence.tick().await;
    loop {
      tokio::select! {
        _ = dirty.tick() => {
          if let Err(e) = all_resources_cache().repair_dirty().await {
            error!("failed dirty resource-cache repair | {e:#}");
          }
        }
        _ = convergence.tick() => {
          if let Err(e) = all_resources_cache().refresh_all().await {
            error!("failed convergence resource-cache refresh | {e:#}");
          }
        }
      }
    }
  });
}
```

Delete the public compatibility function after migrating every writer below;
`rtk rg 'refresh_all_resources_cache' bin/core/src` must have no matches. The
fifteen-second full convergence refresh remains a parallel
cross-Core/uninstrumented-writer backstop, preserving the current worst-case
convergence instead of regressing it to sixty seconds. It does not mark clean
types dirty: clean readers keep their old per-type `Arc` without waiting while
the load is in flight, then switch atomically if the observed generation is
still current. The one-second path repairs only known-dirty types. The P1
improvement is that the eleven convergence loads run in parallel, no
request-path mutation waits for them, dirty reads singleflight, and normal
local writers touch only all actually affected types.

- [ ] **Step 3: Publish create and update results**

At `bin/core/src/resource/mod.rs:547` replace the full refresh with:

```rust
all_resources_cache().upsert::<T>(resource.clone());
```

At `bin/core/src/resource/mod.rs:644`, after `let updated = get::<T>(id_or_name).await?;` and `T::post_update` succeeds, replace the full refresh with:

```rust
all_resources_cache().upsert::<T>(updated.clone());
```

The resource row is already committed before `post_create`, the post-update
`get`, and `post_update` can fail. Attach this exact pattern to every `?`
between the first successful authoritative write and the matching publish:

```rust
.inspect_err(|_| {
  all_resources_cache().mark_dirty::<T>();
})?
```

Do the same around post-delete cleanup after the resource deletion. Thus a
committed create/update/delete can still preserve its existing API error
semantics, but it can never leave a clean stale snapshot. Add fault-injection
tests for failed `post_create` and failed post-update reload; both must leave
the generic `T` dirty. The multi-type delete hooks in Step 7 mark each
dependent type immediately after its own successful write.

Import `all_resources_cache` from `crate::state` and change the generic bounds
on `create`, `update`, `update_meta`, `rename`, and `delete` to
`T: KomodoResource + CachedResource`. In `bin/core/src/sync/mod.rs`, import
`CachedResource` and change the declaration to
`pub trait ResourceSyncTrait: ToToml + CachedResource + Sized`. This is the one
generic caller boundary used by `sync/execute.rs`; all eleven existing
`ResourceSyncTrait` implementations already receive `CachedResource` from
Task 7. Concrete resolver call sites require no additional bounds.

- [ ] **Step 4: Make post-commit meta refresh fail into dirty mode**

After the authoritative `update_one` at `resource/mod.rs:726-728`, replace the full refresh with:

```rust
match get::<T>(id_or_name).await {
  Ok(resource) => all_resources_cache().upsert::<T>(resource),
  Err(e) => {
    all_resources_cache().mark_dirty::<T>();
    error!(
      "resource meta committed but cache refresh failed; using read-through | type: {} | id: {id_or_name} | {e:#}",
      T::resource_type(),
    );
  }
}
```

Return `Ok(())` even when this post-commit refresh fails. This prevents clients from retrying an already committed mutation.

- [ ] **Step 5: Publish rename and delete**

After the successful rename write, mutate the already loaded resource and publish it:

```rust
let mut renamed = resource.clone();
renamed.name.clone_from(&name);
all_resources_cache().upsert::<T>(renamed);
```

After the authoritative delete and `post_delete` work succeeds, replace the full refresh with:

```rust
all_resources_cache().remove::<T>(&resource.id);
```

Require `T: CachedResource` on `update_meta`, `rename`, and `delete`.

- [ ] **Step 6: Mark bulk and info updates dirty**

After `update_info` commits, call:

```rust
all_resources_cache().mark_dirty::<T>();
```

After `remove_tag_from_all` commits, call the same line. The one-second repair loop will republish the affected type; readers use authoritative read-through until then.

- [ ] **Step 7: Inventory and migrate every resource-collection writer**

Before migrating call sites, extract a small `CacheImpact` bitset beside
`AllResourcesCache`. Production writers declare the resource types changed by
each already-authoritative database write, and one `mark_dirty` operation
applies the set. This is production code for Task 8 and is included in its
commit; Task 9 only executes the complete impact matrix.

Create `scripts/performance/audit-resource-cache-writers.sh`:

```sh
#!/usr/bin/env sh
set -eu

root=bin/core/src
out=${1:-target/resource-cache-writers.txt}
mkdir -p "$(dirname "$out")"

rg -n -U --pcre2 \
  '(?s)\.(insert_one|insert_many|update_one|update_many|replace_one|delete_one|delete_many|find_one_and_update|find_one_and_delete|bulk_write)\s*\(' \
  "$root" > "$out.methods"
rg -n \
  'update_one_by_id\s*\(|delete_one_by_id\s*\(|find_one_and_update_by_id\s*\(' \
  "$root" > "$out.by-id"
cat "$out.methods" "$out.by-id" | sort -u > "$out"
rm -f "$out.methods" "$out.by-id"
test -s "$out"
cat "$out"
```

The helper is deliberately repo-wide and receiver-agnostic. It first inventories
every Mongo mutation method, including whitespace-separated and aliased
receivers, then adds all by-ID helpers including generic deletion. Every match
must be classified manually rather than hidden by a collection-shaped regex or
preselected file list. Record the broad inventory count in the evidence file so
a later source-format change cannot silently shrink the audit.

Create `docs/performance/resource-cache-writer-inventory.md` with one row per
match: `source`, `collection`, `mutation`, `cache action`, `failure test`. A
non-resource collection is marked `not applicable`; every resource collection
must either publish the returned row or mark **all** affected types dirty
immediately after each successful database write.

At minimum, cover these non-generic writers discovered at the audited SHA:

- specialized Repo and Deployment rename paths;
- Build, Repo, Stack, and ResourceSync info/config writes in write and execute
  resolvers;
- Server public-key/connection-attempt writes;
- Builder deletion changes Build and Repo rows;
- Server deletion changes Builder, Deployment, Stack, and Repo rows;
- `delete_from_alerters` changes Alerter rows.

For multi-write hooks, mark the type after each individual successful write,
before the next fallible operation. Thus partial success followed by failure
cannot leave a changed dependent collection clean. Generic deletion publishes
the removed `T` and marks Alerter dirty; Builder/Server hooks additionally mark
every dependent type above. Add a fault test for each multi-type hook and a
specialized-writer test for each source file.

Run:

```bash
rtk chmod +x scripts/performance/audit-resource-cache-writers.sh
rtk sh -n scripts/performance/audit-resource-cache-writers.sh
rtk scripts/performance/audit-resource-cache-writers.sh
rtk rg -n 'refresh_all_resources_cache' bin/core/src
```

Expected: the inventory contains every repository match; every row is
classified and tested; the old full-refresh function has no callers or
definition.

- [ ] **Step 8: Convert every global cache consumer to dirty-aware read**

Run: `rtk rg -n 'all_resources_cache\(\)\.load\(\)' bin/core/src`

The three list-item converters need only Repo data. Replace their current
`all_resources_cache().load().repos` branches with these exact blocks so an
unrelated dirty resource type cannot trigger eleven read-throughs:

```rust
// resource/build.rs
match all_resources_cache().read_type::<Repo>().await {
  Ok(repos) => repos
    .get(&build.config.linked_repo)
    .map(|repo| {
      (
        repo.config.git_provider.clone(),
        repo.config.repo.clone(),
        repo.config.branch.clone(),
        repo.config.git_https,
      )
    })
    .unwrap_or(default_git),
  Err(e) => {
    warn!("failed to read linked Repo cache for Build | {e:#}");
    default_git
  }
}

// resource/stack.rs
match all_resources_cache().read_type::<Repo>().await {
  Ok(repos) => repos
    .get(&stack.config.linked_repo)
    .map(|repo| {
      (
        repo.config.git_provider.clone(),
        repo.config.repo.clone(),
        repo.config.branch.clone(),
        repo.config.git_https,
      )
    })
    .unwrap_or(default_git),
  Err(e) => {
    warn!("failed to read linked Repo cache for Stack | {e:#}");
    default_git
  }
}

// resource/sync.rs
match all_resources_cache().read_type::<Repo>().await {
  Ok(repos) => repos
    .get(&resource_sync.config.linked_repo)
    .map(|repo| {
      (
        repo.config.git_provider.clone(),
        repo.config.repo.clone(),
        repo.config.branch.clone(),
        repo.config.git_https,
      )
    })
    .unwrap_or(default_git),
  Err(e) => {
    warn!(
      "failed to read linked Repo cache for ResourceSync | {e:#}"
    );
    default_git
  }
}
```

Ensure `Repo` and `tracing::warn` are imported in each of those modules.
Do not add `.await` inside the existing synchronous sync traits. Instead make
the already-async request boundary load one coherent snapshot and inject it
through the pure conversion/diff functions. Apply these exact signature
changes:

```rust
// sync/mod.rs
fn get_diff(
  original: Self::Config,
  update: Self::PartialConfig,
  all: &AllResourcesById,
) -> anyhow::Result<Self::ConfigDiff>;

// sync/toml.rs
fn replace_ids(
  _resource: &mut Resource<Self::Config, Self::Info>,
  _all: &AllResourcesById,
) {
}

pub fn resource_push_to_toml<R: ToToml>(
  mut resource: Resource<R::Config, R::Info>,
  deploy: bool,
  after: Vec<String>,
  toml: &mut String,
  all_tags: &HashMap<String, Tag>,
  all: &AllResourcesById,
) -> anyhow::Result<()> {
  R::replace_ids(&mut resource, all);
  // Keep the remainder of the existing function unchanged.
}

// helpers/procedure.rs
pub fn replace_procedure_stage_ids_with_names(
  stages: &mut Vec<ProcedureStage>,
  all: &AllResourcesById,
) {
  // Keep the existing replacement loop, using the argument.
}
```

Add the same final `all: &AllResourcesById` argument to
`resource_to_toml`, `sync::view::push_updates_for_view`,
`sync::execute::get_updates_for_execution`, and
`sync::deploy::build_deploy_cache`. Every `ResourceSyncTrait::get_diff`
implementation and every `ToToml::replace_ids` implementation uses that
argument; implementations with no linked IDs name it `_all`. Procedure
implementations pass it to `replace_procedure_stage_ids_with_names`.

At the three outer async paths, acquire exactly one snapshot:

```rust
let all_resources = all_resources_cache()
  .read()
  .await
  .context("failed to read all-resources cache")?;
```

- In `api/execute/sync.rs` and `api/write/sync.rs`, replace their existing
  `AllResourcesById::load()` calls with that block, then pass
  `&all_resources` through the delta/view macros, deploy cache, and user-group
  helpers.
- In `api/read/toml.rs`, load once near `id_to_tags`, then pass the snapshot to
  every macro call, direct `ResourceSync::replace_ids`, and
  `convert_user_groups`.
- Make the async entry points in `sync/user_groups.rs` accept the same borrowed
  snapshot and pass it into `expand_user_group_permissions` and conversion
  helpers. They must not reload it per group.

Finally remove the old `AllResourcesById::load` helper. Task 7 intentionally
added no `AllResourcesCache::load`/`store` compatibility adapters. No
synchronous function performs I/O, and a dirty type is loaded once by its
singleflight owner while concurrent consumers share the published result.

- [ ] **Step 9: Prove no raw global reads remain**

Run:

```bash
rtk rg -n 'all_resources_cache\(\)\.(load|store)\(|AllResourcesById::load\(' bin/core/src
```

Expected: no matches.

Run:

```bash
rtk cargo test -p komodo_core helpers::all_resources
rtk cargo test -p komodo_core resource::refresh
rtk cargo check -p komodo_core
```

Expected: PASS.

Run:

```bash
rtk cargo test -p komodo_core helpers::latest_states::tests::malformed_row_is_unknown_without_poisoning_other_resources -- --exact
rtk cargo test -p komodo_core helpers::latest_states::tests::aggregate_failure_falls_back_to_unknown_for_all_ids -- --exact
```

Expected: both failure semantics tests pass.

- [ ] **Step 10: Run the executable post-commit refresh fault proof**

Add
`resource::refresh::tests::post_commit_refresh_failure_reads_through_and_repairs_within_five_seconds`.
Extract the post-commit reload/publish decision into the smallest test seam used
by `UpdateResourceMeta`; inject one failed reload after a successful fake write.
The next cache read uses the authoritative Server loader, the one-second repair
tick publishes it within a five-second `tokio::time::timeout`, and per-type load
counters prove that no non-Server collection was touched.

Run:

```bash
rtk cargo test -p komodo_core resource::refresh::tests::post_commit_refresh_failure_reads_through_and_repairs_within_five_seconds -- --exact --nocapture
```

Expected: PASS. The write result remains successful, the immediate consumer
observes the authoritative row, the Server snapshot becomes clean within five
seconds, and every other resource-type load counter remains zero.

Before committing Task 8, add the full deterministic `CacheImpact` matrix in
`resource/refresh.rs`: generic create/config/meta/rename/delete, specialized
writers, Builder/Server dependent deletes, partial-hook failure, singleflight,
clean-read, and concurrent-refresh cases enumerated in Task 9 Step 1. These
tests exercise the production impact declarations just introduced, not a
parallel table owned only by tests.

- [ ] **Step 11: Commit mutation-aware resource caching**

```bash
rtk git add bin/core/src/state.rs bin/core/src/resource/refresh.rs bin/core/src/resource/mod.rs bin/core/src/resource/builder.rs bin/core/src/resource/server.rs bin/core/src/connection/mod.rs bin/core/src/connection/server.rs bin/core/src/helpers/all_resources.rs bin/core/src/helpers/procedure.rs bin/core/src/resource/build.rs bin/core/src/resource/stack.rs bin/core/src/resource/sync.rs bin/core/src/api/execute/build.rs bin/core/src/api/execute/repo.rs bin/core/src/api/execute/stack.rs bin/core/src/api/execute/sync.rs bin/core/src/api/write/build.rs bin/core/src/api/write/deployment.rs bin/core/src/api/write/repo.rs bin/core/src/api/write/stack.rs bin/core/src/api/write/sync.rs bin/core/src/api/read/toml.rs bin/core/src/sync/mod.rs bin/core/src/sync/deploy.rs bin/core/src/sync/execute.rs bin/core/src/sync/resources.rs bin/core/src/sync/toml.rs bin/core/src/sync/user_groups.rs bin/core/src/sync/view.rs scripts/performance/audit-resource-cache-writers.sh docs/performance/resource-cache-writer-inventory.md
rtk git commit -m "perf: update resource cache by type"
```

Expected: one commit with no permission behavior changes.

### Task 9: Close checkpoint 3 with cache convergence and rollback proof

**Files:**
- Modify: `docs/performance/core-data-path-budgets.md`
- Create: `scripts/performance/verify-resource-cache-mutations.sh`

- [ ] **Step 1: Re-run the deterministic mutation-impact tests from Task 8**

Task 8 already introduced the production `CacheImpact` declarations and these
tests in `resource/refresh.rs` using injected successful/failing persistence
closures. Do not modify Rust source in this documentation-only checkpoint;
verify that all cases still exist and pass:

- generic create/config/meta/rename/delete impacts `T`, with delete also
  impacting Alerter after `delete_from_alerters`;
- Builder deletion impacts Builder, Build, Repo, and Alerter;
- Server deletion impacts Server, Builder, Deployment, Stack, Repo, and
  Alerter;
- every specialized writer row in the inventory has a matching impact;
- a failure after the first successful dependent write leaves that first type
  dirty;
- 1,000 concurrent dirty readers perform one repair load;
- a clean reader returns the exact old `Arc` without waiting for a convergence
  refresh;
- a concurrent upsert or dirty mark prevents an older convergence load from
  publishing.

- [ ] **Step 2: Create and run the executable assertion wrapper**

Create `scripts/performance/verify-resource-cache-mutations.sh`:

```sh
#!/usr/bin/env sh
set -eu

cargo test -p komodo_core resource::refresh::tests::generic_resource_mutations_touch_expected_types -- --exact
cargo test -p komodo_core resource::refresh::tests::server_delete_dirties_all_changed_types -- --exact
cargo test -p komodo_core resource::refresh::tests::builder_delete_dirties_all_changed_types -- --exact
cargo test -p komodo_core resource::refresh::tests::partial_hook_failure_keeps_completed_writes_dirty -- --exact
cargo test -p komodo_core helpers::all_resources::tests::dirty_reads_are_singleflight -- --exact
cargo test -p komodo_core helpers::all_resources::tests::clean_read_does_not_wait_for_background_refresh -- --exact
cargo test -p komodo_core helpers::all_resources::tests::refresh_does_not_overwrite_concurrent_upsert -- --exact
cargo test -p komodo_core helpers::all_resources::tests::refresh_does_not_overwrite_concurrent_dirty_mark -- --exact
scripts/performance/audit-resource-cache-writers.sh
if rg -n 'refresh_all_resources_cache' bin/core/src; then
  echo "old full-refresh writer path remains" >&2
  exit 1
fi
```

Run:

```bash
rtk chmod +x scripts/performance/verify-resource-cache-mutations.sh
rtk sh -n scripts/performance/verify-resource-cache-mutations.sh
rtk scripts/performance/verify-resource-cache-mutations.sh
```

Expected: each mutation publishes or dirties every type listed in the writer
inventory; no mutation waits for eleven collection reads; clean reads remain
nonblocking during convergence; and a stale refresh cannot overwrite either
an incremental publish or a dirty generation.

- [ ] **Step 3: Record rollback order**

Append:

```markdown
## Resource-cache rollback

1. Deploy the prior reader and writer together; do not mix a raw `ArcSwap<AllResourcesById>` reader with `AllResourcesCache` writers.
2. Restore the fifteen-second full convergence refresh loop.
3. Confirm one full convergence refresh succeeds before removing per-type dirty metrics.
4. Re-run create/update/meta/rename/delete and verify returned authoritative values.

Dirty state is process-local and needs no storage migration.
```

- [ ] **Step 4: Run the checkpoint suite**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo test -p database
rtk cargo test -p komodo_core
```

Expected: all commands exit 0.

- [ ] **Step 5: Commit the rollback runbook**

```bash
rtk git add docs/performance/core-data-path-budgets.md scripts/performance/verify-resource-cache-mutations.sh
rtk git commit -m "docs: add resource cache rollback"
```

Expected: checkpoint 3 is independently deployable.

### Task 10: Add the disabled-by-default Mongo permission generation document

**Files:**
- Create: `lib/database/src/permission_state.rs`
- Modify: `lib/database/src/lib.rs:4-28,45-113`
- Test: `lib/database/src/permission_state.rs`

- [ ] **Step 1: Write the failing default-state test**

Create `lib/database/src/permission_state.rs`:

```rust
#[cfg(test)]
mod tests {
  use super::PermissionCacheState;

  #[test]
  fn cache_starts_disabled_and_clean() {
    let state = PermissionCacheState::default();
    assert_eq!(state.generation, 0);
    assert!(!state.cache_reads_enabled);
    assert!(!state.mutation_in_progress);
    assert!(state.mutation_id.is_none());
  }
}
```

Add `pub mod permission_state;` to `lib/database/src/lib.rs`.

- [ ] **Step 2: Run the test to verify RED**

Run: `rtk cargo test -p database permission_state::tests::cache_starts_disabled_and_clean -- --exact`

Expected: FAIL because `PermissionCacheState` is undefined.

- [ ] **Step 3: Implement the authoritative document type**

Place above the test:

```rust
use serde::{Deserialize, Serialize};

pub const PERMISSION_CACHE_STATE_ID: &str = "global";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionCacheState {
  #[serde(rename = "_id")]
  pub id: String,
  pub generation: i64,
  pub cache_reads_enabled: bool,
  pub mutation_in_progress: bool,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub mutation_id: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub mutation_kind: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub mutation_started_at: Option<i64>,
}

impl Default for PermissionCacheState {
  fn default() -> Self {
    Self {
      id: PERMISSION_CACHE_STATE_ID.to_string(),
      generation: 0,
      cache_reads_enabled: false,
      mutation_in_progress: false,
      mutation_id: None,
      mutation_kind: None,
      mutation_started_at: None,
    }
  }
}
```

- [ ] **Step 4: Register and initialize the typed collection**

Add to `database::Client`:

```rust
pub permission_cache_state: Collection<
  permission_state::PermissionCacheState,
>,
```

Initialize it in `Client::from_database`:

```rust
permission_cache_state: db.collection("PermissionCacheState"),
```

After constructing `client`, initialize the singleton without enabling reads:

```rust
client
  .permission_cache_state
  .update_one(
    doc! {
      "_id": permission_state::PERMISSION_CACHE_STATE_ID
    },
    doc! {
      "$setOnInsert": {
        "generation": 0_i64,
        "cache_reads_enabled": false,
        "mutation_in_progress": false,
      },
    },
  )
  .with_options(
    mungos::mongodb::options::UpdateOptions::builder()
      .upsert(true)
      .build(),
  )
  .await
  .context("failed to initialize permission cache state")?;
```

- [ ] **Step 5: Run database tests to verify GREEN**

Run: `rtk cargo test -p database permission_state`

Expected: PASS.

- [ ] **Step 6: Commit the disabled state document**

```bash
rtk git add lib/database/src/permission_state.rs lib/database/src/lib.rs
rtk git commit -m "feat: add permission generation state"
```

Expected: startup creates one clean, disabled singleton and never enables it implicitly.

### Task 11: Serialize permission mutations with generation CAS

**Files:**
- Create: `bin/core/src/permission/mutation.rs`
- Modify: `bin/core/src/permission.rs:1-38`
- Test: `bin/core/src/permission/mutation.rs`

- [ ] **Step 1: Write failing CAS-filter tests**

Create `bin/core/src/permission/mutation.rs` with:

```rust
#[cfg(test)]
mod tests {
  use database::bson::doc;

  use super::{acquire_filter, finalize_filter};

  #[test]
  fn acquire_requires_clean_expected_generation() {
    assert_eq!(
      acquire_filter(7),
      doc! {
        "_id": "global",
        "generation": 7_i64,
        "mutation_in_progress": false,
      },
    );
  }

  #[test]
  fn finalize_requires_the_acquiring_token() {
    assert_eq!(
      finalize_filter(8, "token-1"),
      doc! {
        "_id": "global",
        "generation": 8_i64,
        "mutation_in_progress": true,
        "mutation_id": "token-1",
      },
    );
  }
}
```

Add `pub(crate) mod mutation;` to `bin/core/src/permission.rs`; API, resource,
and startup sibling modules consume the crate-visible guard.

- [ ] **Step 2: Run the tests to verify RED**

Run: `rtk cargo test -p komodo_core permission::mutation::tests -- --nocapture`

Expected: FAIL because the filter functions do not exist.

- [ ] **Step 3: Implement the mutation token and exact CAS filters**

Place above the tests:

```rust
use std::{ops::AsyncFnOnce, time::Duration};

use anyhow::{Context, anyhow};
use database::{
  bson::{Document, doc},
  mungos::mongodb::options::ReturnDocument,
  permission_state::{
    PERMISSION_CACHE_STATE_ID, PermissionCacheState,
  },
};
use komodo_client::entities::komodo_timestamp;
use strum::{AsRefStr, Display};
use uuid::Uuid;

use crate::state::db_client;

#[derive(Debug, Clone, Copy, Display, AsRefStr)]
pub enum PermissionMutationKind {
  DirectUserPermission,
  GroupPermission,
  UserGroupMembership,
  GroupResourcePermission,
  GroupDeletion,
  UserDeletion,
  ResourceCreationGrant,
  ResourceDeletion,
  UserAdminChange,
  UserBasePermissionChange,
  StartupMigration,
}

#[derive(Debug)]
pub(crate) struct MutationToken {
  id: String,
  generation: i64,
}

pub fn acquire_filter(expected_generation: i64) -> Document {
  doc! {
    "_id": PERMISSION_CACHE_STATE_ID,
    "generation": expected_generation,
    "mutation_in_progress": false,
  }
}

pub fn finalize_filter(
  generation: i64,
  mutation_id: &str,
) -> Document {
  doc! {
    "_id": PERMISSION_CACHE_STATE_ID,
    "generation": generation,
    "mutation_in_progress": true,
    "mutation_id": mutation_id,
  }
}

pub async fn permission_cache_state(
) -> anyhow::Result<PermissionCacheState> {
  db_client()
    .permission_cache_state
    .find_one(doc! { "_id": PERMISSION_CACHE_STATE_ID })
    .await
    .context("failed to read permission cache state")?
    .context("permission cache state is missing")
}
```

- [ ] **Step 4: Implement acquire, finalize, and fail-closed error behavior**

Append:

```rust
async fn acquire(
  kind: PermissionMutationKind,
) -> anyhow::Result<MutationToken> {
  for _ in 0..40 {
    let state = permission_cache_state().await?;
    if state.mutation_in_progress {
      tokio::time::sleep(Duration::from_millis(25)).await;
      continue;
    }
    let id = Uuid::new_v4().to_string();
    let acquired = db_client()
      .permission_cache_state
      .find_one_and_update(
        acquire_filter(state.generation),
        doc! {
          "$inc": { "generation": 1_i64 },
          "$set": {
            "mutation_in_progress": true,
            "mutation_id": &id,
            "mutation_kind": kind.as_ref(),
            "mutation_started_at": komodo_timestamp(),
          },
        },
      )
      .return_document(ReturnDocument::After)
      .await
      .context("failed to acquire permission mutation guard")?;
    if let Some(acquired) = acquired {
      return Ok(MutationToken {
        id,
        generation: acquired.generation,
      });
    }
  }
  Err(anyhow!(
    "permission mutation guard remained busy for 1 second"
  ))
}

async fn finalize(token: &MutationToken) -> anyhow::Result<()> {
  db_client()
    .permission_cache_state
    .find_one_and_update(
      finalize_filter(token.generation, &token.id),
      doc! {
        "$inc": { "generation": 1_i64 },
        "$set": { "mutation_in_progress": false },
        "$unset": {
          "mutation_id": "",
          "mutation_kind": "",
          "mutation_started_at": "",
        },
      },
    )
    .return_document(ReturnDocument::After)
    .await
    .context("failed to finalize permission mutation guard")?
    .context("permission mutation guard CAS was lost")?;
  Ok(())
}

pub async fn with_permission_mutation<T>(
  kind: PermissionMutationKind,
  mutation: impl AsyncFnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
  let token = acquire(kind).await?;
  let value = mutation().await.with_context(|| {
    format!(
      "permission mutation failed; cache remains bypassed under token {}",
      token.id
    )
  })?;
  finalize(&token).await.with_context(|| {
    format!(
      "permission mutation committed but generation finalization failed; cache remains bypassed under token {}",
      token.id
    )
  })?;
  Ok(value)
}
```

Do not add an async cleanup in `Drop`. A failed closure or lost finalize CAS intentionally leaves `mutation_in_progress=true` until explicit recovery.

- [ ] **Step 5: Run CAS tests to verify GREEN**

Run: `rtk cargo test -p komodo_core permission::mutation::tests`

Expected: both tests PASS.

- [ ] **Step 6: Commit the central mutation protocol**

```bash
rtk git add bin/core/src/permission.rs bin/core/src/permission/mutation.rs
rtk git commit -m "feat: guard permission mutations with CAS"
```

Expected: no mutation endpoint uses the helper yet, so permission cache reads remain disabled.

### Task 12: Route every effective-permission mutation through the guard

**Files:**
- Modify: `bin/core/Cargo.toml`
- Modify: `bin/core/src/permission/mutation.rs`
- Modify: `bin/core/src/api/write/permissions.rs:24-293`
- Modify: `bin/core/src/api/write/user_group.rs:103-356`
- Modify: `bin/core/src/api/write/user.rs:187-258`
- Modify: `bin/core/src/helpers/mod.rs:200-234`
- Modify: `bin/core/src/resource/mod.rs:473-552,821-945`
- Modify: `bin/core/src/startup.rs:524-582`
- Create: `scripts/performance/permission-cache-control.js`
- Create: `scripts/performance/audit-permission-writers.sh`
- Test: the same resolver modules plus staging MongoDB

- [ ] **Step 1: Add a regression test for failed mutations**

In `permission/mutation.rs`, extract the closure/finalize sequencing into a testable helper and add:

```rust
#[tokio::test]
async fn failed_permission_write_does_not_finalize_guard() {
  let finalized = std::sync::Arc::new(
    std::sync::atomic::AtomicBool::new(false),
  );
  let result = run_guarded(
    || async { anyhow::bail!("injected write failure") },
    {
      let finalized = finalized.clone();
      move || async move {
        finalized.store(
          true,
          std::sync::atomic::Ordering::Release,
        );
        Ok(())
      }
    },
  )
  .await;
  assert!(result.is_err());
  assert!(!finalized.load(
    std::sync::atomic::Ordering::Acquire,
  ));
}
```

Run: `rtk cargo test -p komodo_core permission::mutation::tests::failed_permission_write_does_not_finalize_guard -- --exact`

Expected: FAIL because `run_guarded` is undefined.

- [ ] **Step 2: Implement the tested sequencing and use it from the Mongo wrapper**

Add:

```rust
async fn run_guarded<T>(
  mutation: impl AsyncFnOnce() -> anyhow::Result<T>,
  finalize: impl AsyncFnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<T> {
  let value = mutation().await?;
  finalize().await?;
  Ok(value)
}
```

Change `with_permission_mutation` to call `run_guarded(mutation, || finalize(&token))` and retain the existing token context on both errors.

At the same step, make guard storage injectable without duplicating resolver
logic:

```rust
pub(crate) trait PermissionMutationBackend: Send + Sync {
  async fn acquire(
    &self,
    kind: PermissionMutationKind,
  ) -> anyhow::Result<MutationToken>;
  async fn finalize(
    &self,
    token: &MutationToken,
  ) -> anyhow::Result<()>;
}

pub(crate) async fn with_permission_mutation_using<B, T>(
  backend: &B,
  kind: PermissionMutationKind,
  mutation: impl AsyncFnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T>
where
  B: PermissionMutationBackend,
{
  let token = backend.acquire(kind).await?;
  run_guarded(
    mutation,
    || backend.finalize(&token),
  )
  .await
}
```

`MongoPermissionMutationBackend` owns the current CAS implementation, exposes a
`pub(crate)` zero-sized value/accessor, and the public wrapper delegates to it.
The test backend records
`acquire -> write labels -> finalize`, exposes busy/acquire/finalize failures,
and stores the simulated state used by disable/recovery races. In each owning
resolver/helper module, extract only the smallest `*_guarded_with` function,
mark it `#[doc(hidden)] pub(crate)`, and have it accept
`&impl PermissionMutationBackend` plus the actual
database-write closure. The production `Resolve` implementation and the matrix
test call that same function; a second mock implementation of mutation routing
is forbidden.

Add an empty-by-default Cargo feature
`p1-permission-finalize-failpoint`. Behind it, the Mongo backend supports the
one-shot environment value
`KOMODO_P1_FAIL_PERMISSION_FINALIZE_ONCE=<PermissionMutationKind>`. Startup
requires `KOMODO_P1_ALLOW_PERMISSION_FAILPOINT=1` and a resolved database name
starting with `p1_`; otherwise Core exits before listening. The matching
finalize call atomically consumes the one shot and returns an injected error
before its CAS, leaving the real token held for Task 14 recovery. Normal and
release builds without the feature contain no failpoint branch.

The feature contract has two literal INFO/ERROR message bodies, emitted once
without credentials or database URI:

```text
permission finalize failpoint armed: DirectUserPermission
permission finalize failpoint consumed: DirectUserPermission
```

Use the same fixed prefix with the selected mutation kind for other unit cases.
Task 14 searches these complete message bodies with `rg -F`; changing either
string requires changing the live verifier in the same commit.

Run the focused test again.

Expected: PASS.

- [ ] **Step 3: Guard direct, group, membership, user-state, and deletion writes**

Wrap the authoritative database operations—not validation reads—with `with_permission_mutation` using this exact mapping:

| Resolver/helper | Kind | Operations inside one closure |
|---|---|---|
| `UpdateUserAdmin` | `UserAdminChange` | user `admin` update |
| `UpdateUserBasePermissions` | `UserBasePermissionChange` | user `enabled`, `create_server_permissions`, and `create_build_permissions` update |
| `UpdatePermissionOnResourceType` with `User` | `DirectUserPermission` | `users.all.<type>` update |
| `UpdatePermissionOnResourceType` with `UserGroup` | `GroupPermission` | `user_groups.all.<type>` update |
| `UpdatePermissionOnTarget` with `User` | `DirectUserPermission` | permission upsert |
| `UpdatePermissionOnTarget` with `UserGroup` | `GroupResourcePermission` | permission upsert |
| `AddUserToUserGroup`, `RemoveUserFromUserGroup`, `SetUsersInUserGroup`, `SetEveryoneUserGroup` | `UserGroupMembership` | membership/everyone update and result read |
| `DeleteUserGroup` | `GroupDeletion` | group delete plus its permission-row delete |
| `DeleteUser` | `UserDeletion` | user delete, permission-row delete, and pull from every group |
| generic `create<T>` for a non-admin creator | `ResourceCreationGrant` | preallocate resource ObjectId, insert creator permission first, then insert the authoritative resource with that ObjectId |
| generic `delete<T>` | `ResourceDeletion` | authoritative resource delete first, then permission-row delete |
| `clean_up_server_templates` | `StartupMigration` | permission/user/group legacy cleanup as one guarded closure |

The wrapper form is:

```rust
with_permission_mutation(
  PermissionMutationKind::UserGroupMembership,
  async || {
    db.user_groups
      .update_one(filter.clone(), update)
      .await
      .context("failed to update user-group membership")?;
    db.user_groups
      .find_one(filter)
      .await
      .context("failed to read updated user group")?
      .context("updated user group disappeared")
  },
)
.await
```

For user deletion, add this missing cleanup inside the same closure:

```rust
db.permissions
  .delete_many(doc! {
    "user_target.type": "User",
    "user_target.id": &user.id,
  })
  .await
  .context("failed to delete user permission rows")?;
```

Do not acquire a nested guard in `create_permission`. For a non-admin creator,
generic `create<T>` preallocates the resource ObjectId, acquires
`ResourceCreationGrant`, inserts the creator permission referencing that ID
first, then inserts the authoritative resource with the same ID inside one
closure. Return the resource only after guard finalization. If the second write
fails, the remaining permission is a reference-only orphan that the recovery
invariant scan must detect and refuse to clear; the inverse order could leave a
resource missing its required creator grant with no orphan to detect. An admin
creation has no creator permission and its new `base_permission` is `None`, so
it skips this guard and performs only the existing resource insert. Add a
non-admin busy/acquire-failure test asserting neither write commits, a
permission-first assertion, and a resource-insert failure test that leaves the
orphan permission plus guard set for recovery. Add an admin test asserting no
guard/backend call and no permission row.

Inside generic `delete<T>`, delete the authoritative resource first and its
permission rows second. While the guard is set, authorization already bypasses
snapshots; if permission cleanup fails after the resource delete, the remaining
permission rows are detectable orphans and the recovery invariant scan must
refuse to clear the token. The inverse order is forbidden because a failed
resource delete could silently lose valid grants while leaving no orphan for
recovery to detect. Add a partial-delete recovery test for this order.

Change the unguarded inner `create_permission` and
`delete_all_permissions_on_resource` primitives to return
`anyhow::Result<()>`. Propagate their results inside the owning closure; do not
log-and-continue or reacquire the guard.

- [ ] **Step 4: Verify sync mutations are covered through resolvers**

Run:

```bash
rtk rg -n 'UpdatePermissionOn(ResourceType|Target)|SetUsersInUserGroup|SetEveryoneUserGroup|DeleteUserGroup' bin/core/src/sync/user_groups.rs
```

Expected: sync uses the guarded resolvers. Do not add a second outer guard around `run_updates`; nested guards would deadlock.

- [ ] **Step 5: Create the operational kill-switch and recovery script**

Create `scripts/performance/permission-cache-control.js`:

```javascript
const action = process.env.PERMISSION_CACHE_ACTION;
const state = db.PermissionCacheState.findOne({ _id: "global" });
if (!state) throw new Error("PermissionCacheState/global is missing");

if (action === "disable") {
  let disabled = null;
  for (let attempt = 0; attempt < 20 && !disabled; attempt += 1) {
    const current = db.PermissionCacheState.findOne({ _id: "global" });
    if (!current) throw new Error("PermissionCacheState/global disappeared");
    const filter = {
      _id: "global",
      generation: current.generation,
      mutation_in_progress: current.mutation_in_progress,
    };
    let update;
    if (current.mutation_in_progress) {
      if (!current.mutation_id) {
        throw new Error("held mutation guard has no mutation_id");
      }
      filter.mutation_id = current.mutation_id;
      // Preserve the owner's generation/token so its finalize CAS can still
      // succeed. Finalize changes only guard fields and therefore preserves
      // this disabled value.
      update = { $set: { cache_reads_enabled: false } };
    } else {
      update = {
        $inc: { generation: NumberLong(1) },
        $set: { cache_reads_enabled: false },
      };
    }
    disabled = db.PermissionCacheState.findOneAndUpdate(filter, update);
  }
  if (!disabled) throw new Error("disable CAS did not converge");
} else if (action === "enable") {
  const updated = db.PermissionCacheState.findOneAndUpdate(
    {
      _id: "global",
      mutation_in_progress: false,
    },
    {
      $inc: { generation: NumberLong(1) },
      $set: { cache_reads_enabled: true },
    },
  );
  if (!updated) throw new Error("cannot enable while mutation guard is set");
} else if (action === "recover") {
  const token = process.env.PERMISSION_MUTATION_ID;
  if (!token) throw new Error("PERMISSION_MUTATION_ID is required");
  const generationText = process.env.PERMISSION_MUTATION_GENERATION;
  if (!generationText) {
    throw new Error("PERMISSION_MUTATION_GENERATION is required");
  }
  const generation = NumberLong(generationText);

  const resourceCollections = {
    Swarm: "Swarm",
    Server: "Server",
    Stack: "Stack",
    Deployment: "Deployment",
    Build: "Build",
    Repo: "Repo",
    Procedure: "Procedure",
    Action: "Action",
    ResourceSync: "ResourceSync",
    Builder: "Builder",
    Alerter: "Alerter",
  };
  for (const permission of db.Permission.find()) {
    const userCollection =
      permission.user_target.type === "User"
        ? db.User
        : db.UserGroup;
    if (!userCollection.findOne({ _id: ObjectId(permission.user_target.id) })) {
      throw new Error("orphan permission user target: " + permission._id);
    }
    if (permission.resource_target.type !== "System") {
      const collection =
        resourceCollections[permission.resource_target.type];
      if (!collection) {
        throw new Error("unknown resource target: " + permission._id);
      }
      if (!db.getCollection(collection).findOne({
        _id: ObjectId(permission.resource_target.id),
      })) {
        throw new Error("orphan permission resource target: " + permission._id);
      }
    }
  }
  for (const group of db.UserGroup.find()) {
    for (const userId of group.users) {
      if (!db.User.findOne({ _id: ObjectId(userId) })) {
        throw new Error("orphan user-group member: " + group._id);
      }
    }
  }
  const recovered = db.PermissionCacheState.findOneAndUpdate(
    {
      _id: "global",
      generation,
      mutation_in_progress: true,
      mutation_id: token,
    },
    {
      $inc: { generation: NumberLong(1) },
      $set: {
        mutation_in_progress: false,
        cache_reads_enabled: false,
      },
      $unset: {
        mutation_id: "",
        mutation_kind: "",
        mutation_started_at: "",
      },
    },
  );
  if (!recovered) throw new Error("recovery CAS did not match");
} else {
  throw new Error("PERMISSION_CACHE_ACTION must be disable, enable, or recover");
}

print(EJSON.stringify(db.PermissionCacheState.findOne({ _id: "global" })));
```

Recovery deliberately leaves snapshot reads disabled. Re-enable only after the invariant scan and cross-Core tests pass.

Add matching Rust recovery-contract helpers in `permission/mutation.rs`; keep
the filter and update fields identical to the operational script:

```rust
pub fn recovery_filter(
  generation: i64,
  mutation_id: &str,
) -> Document {
  doc! {
    "_id": PERMISSION_CACHE_STATE_ID,
    "generation": generation,
    "mutation_in_progress": true,
    "mutation_id": mutation_id,
  }
}

pub fn recovery_update() -> Document {
  doc! {
    "$inc": { "generation": 1_i64 },
    "$set": {
      "mutation_in_progress": false,
      "cache_reads_enabled": false,
    },
    "$unset": {
      "mutation_id": "",
      "mutation_kind": "",
      "mutation_started_at": "",
    },
  }
}
```

Extend the existing `permission::mutation::tests` module with the exact tests
invoked by Task 14:

```rust
#[test]
fn wrong_recovery_token_is_rejected() {
  let held = doc! {
    "_id": "global",
    "generation": 8_i64,
    "mutation_in_progress": true,
    "mutation_id": "token-1",
  };
  assert_eq!(recovery_filter(8, "token-1"), held);
  assert_ne!(recovery_filter(8, "wrong-token"), held);
}

#[test]
fn successful_recovery_disables_reads_and_advances_generation() {
  assert_eq!(
    recovery_update(),
    doc! {
      "$inc": { "generation": 1_i64 },
      "$set": {
        "mutation_in_progress": false,
        "cache_reads_enabled": false,
      },
      "$unset": {
        "mutation_id": "",
        "mutation_kind": "",
        "mutation_started_at": "",
      },
    },
  );
}
```

Import `recovery_filter` and `recovery_update` beside the existing test imports.

Also extract Rust `disable_filter_and_update(&PermissionCacheState)` builders
whose clean/held branches are field-for-field identical to the JavaScript CAS.
Add `disable_during_held_mutation_preserves_finalize_token`: acquire generation
8/token A in the fake store, apply disable, assert generation/token are still
8/A, then apply the real `finalize_filter(8, "A")`; final state has generation
9, no guard, and `cache_reads_enabled=false`. Add the opposite interleaving
where finalize wins before disable; disable retries from generation 9 and
advances to 10. Neither interleaving may leave a stuck guard or re-enable
reads. A clean-state test proves disable advances generation exactly once.

- [ ] **Step 6: Prove there are no unguarded repository mutation paths**

Create `scripts/performance/audit-permission-writers.sh`:

```sh
#!/usr/bin/env sh
set -eu

root=bin/core/src
out=${1:-target/permission-writers.txt}
mkdir -p "$(dirname "$out")"
rg -n -U --pcre2 \
  '(?s)\.(insert_one|insert_many|update_one|update_many|replace_one|delete_one|delete_many|find_one_and_update|find_one_and_delete|bulk_write)\s*\(' \
  "$root" > "$out.methods"
rg -n \
  'update_one_by_id\s*\(|delete_one_by_id\s*\(|find_one_and_update_by_id\s*\(|create_permission|delete_all_permissions_on_resource' \
  "$root" > "$out.helpers"
cat "$out.methods" "$out.helpers" | sort -u > "$out"
rm -f "$out.methods" "$out.helpers"
test -s "$out"
cat "$out"
```

Run:

```bash
rtk chmod +x scripts/performance/audit-permission-writers.sh
rtk sh -n scripts/performance/audit-permission-writers.sh
rtk scripts/performance/audit-permission-writers.sh
```

Expected: the receiver-agnostic repo-wide output is attached to the PR and
every Mongo mutation match is
classified as a guarded effective-permission mutation, a fresh user ID with no
pre-existing snapshot, a non-authorizing profile field, or initialization. A
collection-prefiltered or preselected source-file search is not accepted. In
particular, the inventory must contain direct writes in
`api/write/permissions.rs` and generic `delete_one_by_id` call sites; show that
`UpdateUserBasePermissions` and `UpdateUserAdmin` are guarded, while
`CreateLocalUser`, OAuth user creation, and `CreateServiceUser` introduce fresh
IDs with no earlier snapshot to invalidate.

Before committing, add `permission::mutation::matrix` using the injected
backend and the real module-local `*_guarded_with` functions. Cover direct and
group grants, all membership variants, group/user deletion, resource
create/delete, admin/enable/capability changes, clean and held disable races,
failure after each successful write, and recovery CAS. Assert resource-create
acquire failure writes zero rows and partial resource deletion leaves the
permission rows as detectable orphans. This matrix is production-routing proof
for Task 12; Task 14 only reruns it and adds the live two-Core/failpoint proof.

- [ ] **Step 7: Run tests and commit guarded writers**

Run:

```bash
rtk cargo test -p komodo_core permission::mutation
rtk cargo test -p komodo_core permission::mutation::matrix -- --nocapture
rtk cargo test -p komodo_core permission::mutation::tests::wrong_recovery_token_is_rejected -- --exact
rtk cargo test -p komodo_core permission::mutation::tests::successful_recovery_disables_reads_and_advances_generation -- --exact
rtk cargo test -p komodo_core permission::mutation::tests::disable_during_held_mutation_preserves_finalize_token -- --exact
rtk cargo test -p komodo_core
```

Expected: PASS.

Commit:

```bash
rtk git add bin/core/Cargo.toml bin/core/src/api/write/permissions.rs bin/core/src/api/write/user_group.rs bin/core/src/api/write/user.rs bin/core/src/helpers/mod.rs bin/core/src/resource/mod.rs bin/core/src/startup.rs bin/core/src/permission/mutation.rs scripts/performance/permission-cache-control.js scripts/performance/audit-permission-writers.sh
rtk git commit -m "feat: guard effective permission writes"
```

### Task 13: Build generation-keyed permission snapshots and targeted queries

**Files:**
- Create: `bin/core/src/permission/snapshot.rs`
- Modify: `bin/core/src/permission.rs:1-38,85-560`
- Modify: `bin/core/src/api/read/update.rs:19-37`
- Modify: `bin/core/src/api/read/alert.rs:15-35`
- Modify: `bin/core/src/api/read/staging.rs`
- Modify: `scripts/performance/profile-core-resolver-commands.sh`
- Create: `scripts/performance/profile-cold-permission-snapshot.sh`
- Test: `bin/core/src/permission/snapshot.rs`

- [ ] **Step 1: Write failing scope and targeted-query tests**

Create `bin/core/src/permission/snapshot.rs` with tests:

```rust
#[cfg(test)]
mod tests {
  use std::collections::HashSet;

  use database::bson::doc;
  use komodo_client::entities::{
    ResourceTarget, ResourceTargetVariant,
    permission::{Permission, PermissionLevel, UserTarget},
    user::User,
    user_group::UserGroup,
  };

  use super::{
    PermissionSnapshotInputs, ResourceBasePermission,
    ResourceReadScope, UserPermissionSnapshot,
    exact_target_variant,
  };

  #[test]
  fn exact_target_query_limits_authorization_to_one_type() {
    assert_eq!(
      exact_target_variant(&doc! {
        "target.type": "Server",
        "target.id": "server-1",
      }),
      Some(ResourceTargetVariant::Server),
    );
    assert_eq!(
      exact_target_variant(&doc! {
        "$or": [
          { "target.type": "Server" },
          { "target.type": "Build" },
        ],
      }),
      None,
    );
  }

  #[test]
  fn snapshot_denies_ids_outside_the_scope() {
    let mut snapshot = UserPermissionSnapshot::default();
    snapshot.enabled = true;
    snapshot.scopes.insert(
      ResourceTargetVariant::Server,
      ResourceReadScope::Ids(
        HashSet::from(["server-1".to_string()]),
      ),
    );
    assert!(snapshot.can_read(&ResourceTarget::Server("server-1".into())));
    assert!(!snapshot.can_read(&ResourceTarget::Server("server-2".into())));
  }

  #[test]
  fn current_admin_can_read_system_but_disabled_user_cannot() {
    let mut snapshot = UserPermissionSnapshot {
      enabled: true,
      unrestricted: true,
      ..Default::default()
    };
    assert!(snapshot.can_read(&ResourceTarget::System(
      "system".into(),
    )));
    snapshot.enabled = false;
    assert!(!snapshot.can_read(&ResourceTarget::System(
      "system".into(),
    )));
  }
}
```

Add `mod snapshot;` and the public re-export below to `permission.rs`:

```rust
pub use snapshot::{
  PermissionSnapshotProvider, permission_snapshot_provider,
};
```

- [ ] **Step 2: Run the tests to verify RED**

Run: `rtk cargo test -p komodo_core permission::snapshot::tests`

Expected: FAIL because the scope types and parser are undefined.

- [ ] **Step 3: Implement snapshot types and the Plan 2 provider contract**

Add above the tests:

```rust
use std::{
  collections::{HashMap, HashSet},
  sync::{
    Arc, OnceLock,
    atomic::{AtomicU64, Ordering},
  },
  time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use database::bson::{Document, doc};
use futures_util::TryStreamExt;
use komodo_client::entities::{
  ResourceTarget, ResourceTargetVariant,
  permission::{
    Permission, PermissionLevel, PermissionLevelAndSpecifics,
    UserTarget,
  },
  user::User,
  user_group::UserGroup,
};
use serde::Deserialize;
use tokio::sync::{OnceCell, RwLock};

use crate::{
  config::core_config,
  helpers::query::{
    get_user, get_user_permission_on_target, get_user_user_groups,
  },
  permission::mutation::permission_cache_state,
  state::db_client,
};

use super::authoritative_user_resource_target_query;

#[derive(Clone, Debug)]
pub enum ResourceReadScope {
  All,
  Ids(HashSet<String>),
}

#[derive(Clone, Debug, Deserialize)]
struct ResourceBasePermission {
  resource_type: ResourceTargetVariant,
  resource_id: String,
  base_permission: PermissionLevelAndSpecifics,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PermissionSnapshotInput {
  Permission { permission: Permission },
  ResourceBase {
    resource_type: ResourceTargetVariant,
    resource_id: String,
    base_permission: PermissionLevelAndSpecifics,
  },
}

#[derive(Debug)]
struct PermissionSnapshotInputs {
  user_ids: HashMap<
    String,
    HashMap<ResourceTargetVariant, HashSet<String>>,
  >,
  group_ids: HashMap<
    String,
    HashMap<ResourceTargetVariant, HashSet<String>>,
  >,
  base_ids: HashMap<ResourceTargetVariant, HashSet<String>>,
}

#[derive(Clone, Debug, Default)]
pub struct UserPermissionSnapshot {
  pub generation: i64,
  pub enabled: bool,
  pub unrestricted: bool,
  pub scopes: HashMap<ResourceTargetVariant, ResourceReadScope>,
}

impl UserPermissionSnapshot {
  pub fn can_read(&self, target: &ResourceTarget) -> bool {
    if !self.enabled {
      return false;
    }
    if self.unrestricted {
      return true;
    }
    let (variant, id) = target.extract_variant_id();
    match self.scopes.get(&variant) {
      Some(ResourceReadScope::All) => true,
      Some(ResourceReadScope::Ids(ids)) => ids.contains(id),
      None => false,
    }
  }
}

const MAX_USER_SNAPSHOTS_PER_GENERATION: usize = 4_096;
const USER_SNAPSHOT_IDLE_TTL_MS: u64 = 15 * 60 * 1_000;

fn now_unix_ms() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_millis() as u64
}

struct CachedUserSnapshot {
  cell: OnceCell<Arc<UserPermissionSnapshot>>,
  last_access_ms: AtomicU64,
}

impl CachedUserSnapshot {
  fn new(now_ms: u64) -> Self {
    Self {
      cell: OnceCell::new(),
      last_access_ms: AtomicU64::new(now_ms),
    }
  }
}

fn prune_user_snapshots(
  snapshots: &mut HashMap<String, Arc<CachedUserSnapshot>>,
  now_ms: u64,
) {
  snapshots.retain(|_, entry| {
    now_ms.saturating_sub(
      entry.last_access_ms.load(Ordering::Acquire),
    ) <= USER_SNAPSHOT_IDLE_TTL_MS
  });
  if snapshots.len() < MAX_USER_SNAPSHOTS_PER_GENERATION {
    return;
  }
  let remove = snapshots.len()
    - MAX_USER_SNAPSHOTS_PER_GENERATION
    + 1;
  let mut oldest = snapshots
    .iter()
    .map(|(id, entry)| {
      (
        id.clone(),
        entry.last_access_ms.load(Ordering::Acquire),
      )
    })
    .collect::<Vec<_>>();
  oldest.sort_unstable_by_key(|(_, last_access)| *last_access);
  for (id, _) in oldest.into_iter().take(remove) {
    snapshots.remove(&id);
  }
}

struct PermissionSnapshotGeneration {
  generation: i64,
  inputs: OnceCell<Arc<PermissionSnapshotInputs>>,
  snapshots: RwLock<HashMap<String, Arc<CachedUserSnapshot>>>,
}

impl PermissionSnapshotGeneration {
  fn new(generation: i64) -> Self {
    Self {
      generation,
      inputs: OnceCell::new(),
      snapshots: RwLock::new(HashMap::new()),
    }
  }
}

#[derive(Default)]
pub struct PermissionSnapshotProvider {
  current: RwLock<Option<Arc<PermissionSnapshotGeneration>>>,
}

pub fn permission_snapshot_provider(
) -> &'static PermissionSnapshotProvider {
  static PROVIDER: OnceLock<PermissionSnapshotProvider> =
    OnceLock::new();
  PROVIDER.get_or_init(Default::default)
}

pub fn exact_target_variant(
  query: &Document,
) -> Option<ResourceTargetVariant> {
  query
    .get_str("target.type")
    .ok()
    .and_then(|value| value.parse().ok())
}

fn deny_query() -> Document {
  doc! { "_id": { "$exists": false } }
}

fn scope_clause(
  snapshot: &UserPermissionSnapshot,
  variant: ResourceTargetVariant,
) -> Document {
  match snapshot.scopes.get(&variant) {
    Some(ResourceReadScope::All) => {
      doc! { "target.type": variant.as_ref() }
    }
    Some(ResourceReadScope::Ids(ids)) => doc! {
      "target.type": variant.as_ref(),
      "target.id": {
        "$in": ids.iter().cloned().collect::<Vec<_>>(),
      },
    },
    None => deny_query(),
  }
}

fn snapshot_access_query(
  snapshot: &UserPermissionSnapshot,
  incoming_query: Option<&Document>,
) -> Document {
  if let Some(variant) = incoming_query.and_then(exact_target_variant) {
    return scope_clause(snapshot, variant);
  }
  let clauses = [
    ResourceTargetVariant::Swarm,
    ResourceTargetVariant::Server,
    ResourceTargetVariant::Stack,
    ResourceTargetVariant::Deployment,
    ResourceTargetVariant::Build,
    ResourceTargetVariant::Repo,
    ResourceTargetVariant::Procedure,
    ResourceTargetVariant::Action,
    ResourceTargetVariant::ResourceSync,
    ResourceTargetVariant::Builder,
    ResourceTargetVariant::Alerter,
  ]
  .into_iter()
  .map(|variant| scope_clause(snapshot, variant))
  .collect::<Vec<_>>();
  doc! { "$or": clauses }
}

fn combine_query(
  access: Document,
  incoming: Option<Document>,
) -> Option<Document> {
  Some(match incoming {
    Some(incoming) => doc! { "$and": [access, incoming] },
    None => access,
  })
}

async fn authoritative_query_for_session(
  session_user: &User,
  incoming_query: Option<Document>,
) -> anyhow::Result<Option<Document>> {
  let current = get_user(&session_user.id).await?;
  if !current.enabled {
    return Ok(Some(deny_query()));
  }
  authoritative_user_resource_target_query(
    &current,
    incoming_query,
  )
  .await
}

async fn authoritative_can_read_for_session(
  session_user: &User,
  target: &ResourceTarget,
) -> anyhow::Result<bool> {
  let current = get_user(&session_user.id).await?;
  if !current.enabled {
    return Ok(false);
  }
  if current.admin || core_config().transparent_mode {
    return Ok(true);
  }
  Ok(
    get_user_permission_on_target(&current, target)
      .await?
      .fulfills(&PermissionLevel::Read.into()),
  )
}
```

This exact public contract is Merge Gate A for Plan 2:

```rust
impl PermissionSnapshotProvider {
  pub async fn can_read_target(
    &self,
    user: &User,
    target: &ResourceTarget,
  ) -> anyhow::Result<bool>;
}
```

`can_read_target` owns the complete double-read authorization linearization
point. Plan 2 may call it once for one user and one event inside its serialized
fan-out hub, then deliver that single decision to that user's current
connections. It must call the provider again for the next event and must not
introduce a second cross-event WebSocket permission cache.

- [ ] **Step 4: Build one snapshot from authoritative permission and resource rows**

Add one `$unionWith` aggregation so resource `base_permission` never depends
on a process-local resource-cache convergence interval:

```rust
fn resource_base_pipeline(
  resource_type: ResourceTargetVariant,
) -> Vec<Document> {
  vec![
    doc! {
      "$match": {
        "base_permission.level": {
          "$in": ["Read", "Execute", "Write"],
        },
      },
    },
    doc! {
      "$project": {
        "_id": 0,
        "kind": { "$literal": "resource_base" },
        "resource_type": {
          "$literal": resource_type.as_ref(),
        },
        "resource_id": { "$toString": "$_id" },
        "base_permission": 1,
      },
    },
  ]
}

fn snapshot_input_pipeline() -> Vec<Document> {
  let mut pipeline = vec![
    doc! {
      "$match": {
        "level": { "$in": ["Read", "Execute", "Write"] },
      },
    },
    doc! {
      "$project": {
        "_id": 0,
        "kind": { "$literal": "permission" },
        "permission": "$$ROOT",
      },
    },
  ];
  for (collection, resource_type) in [
    ("Server", ResourceTargetVariant::Server),
    ("Swarm", ResourceTargetVariant::Swarm),
    ("Stack", ResourceTargetVariant::Stack),
    ("Deployment", ResourceTargetVariant::Deployment),
    ("Build", ResourceTargetVariant::Build),
    ("Repo", ResourceTargetVariant::Repo),
    ("Procedure", ResourceTargetVariant::Procedure),
    ("Action", ResourceTargetVariant::Action),
    ("ResourceSync", ResourceTargetVariant::ResourceSync),
    ("Builder", ResourceTargetVariant::Builder),
    ("Alerter", ResourceTargetVariant::Alerter),
  ] {
    pipeline.push(doc! {
      "$unionWith": {
        "coll": collection,
        "pipeline": resource_base_pipeline(resource_type),
      },
    });
  }
  pipeline
}

async fn load_snapshot_inputs(
) -> anyhow::Result<PermissionSnapshotInputs> {
  let mut cursor = db_client()
    .permissions
    .aggregate(snapshot_input_pipeline())
    .with_options(
      AggregateOptions::builder()
        .batch_size(20_000)
        .comment(Bson::String(
          "p1:permission-snapshot-inputs".into(),
        ))
        .build(),
    )
    .await
    .context("failed to aggregate permission snapshot inputs")?;
  let mut permissions = Vec::new();
  let mut base_permissions = Vec::new();
  while let Some(document) = cursor.try_next().await? {
    match database::bson::from_document(document)? {
      PermissionSnapshotInput::Permission { permission } => {
        permissions.push(permission);
      }
      PermissionSnapshotInput::ResourceBase {
        resource_type,
        resource_id,
        base_permission,
      } => base_permissions.push(ResourceBasePermission {
        resource_type,
        resource_id,
        base_permission,
      }),
    }
  }
  Ok(PermissionSnapshotInputs::from_rows(
    permissions,
    base_permissions,
  ))
}

impl PermissionSnapshotInputs {
  fn from_rows(
    permissions: Vec<Permission>,
    base_permissions: Vec<ResourceBasePermission>,
  ) -> Self {
    let mut inputs = Self {
      user_ids: HashMap::new(),
      group_ids: HashMap::new(),
      base_ids: HashMap::new(),
    };
    for permission in permissions {
      if permission.level < PermissionLevel::Read {
        continue;
      }
      let (variant, resource_id) =
        permission.resource_target.extract_variant_id();
      let resource_id = resource_id.to_string();
      let target = match permission.user_target {
        UserTarget::User(id) => inputs.user_ids.entry(id),
        UserTarget::UserGroup(id) => inputs.group_ids.entry(id),
      };
      target
        .or_default()
        .entry(variant)
        .or_default()
        .insert(resource_id);
    }
    for resource in base_permissions {
      if resource.base_permission.level >= PermissionLevel::Read {
        inputs
          .base_ids
          .entry(resource.resource_type)
          .or_default()
          .insert(resource.resource_id);
      }
    }
    inputs
  }
}

impl PermissionSnapshotProvider {
  async fn generation(
    &self,
    generation: i64,
  ) -> Arc<PermissionSnapshotGeneration> {
    if let Some(current) = self.current.read().await.as_ref()
      && current.generation == generation
    {
      return current.clone();
    }
    let mut current = self.current.write().await;
    if let Some(existing) = current.as_ref()
      && existing.generation == generation
    {
      return existing.clone();
    }
    if current
      .as_ref()
      .is_some_and(|existing| existing.generation > generation)
    {
      // An older in-flight authorization must not replace a newer epoch.
      return Arc::new(PermissionSnapshotGeneration::new(generation));
    }
    let next = Arc::new(PermissionSnapshotGeneration::new(generation));
    *current = Some(next.clone());
    next
  }

  async fn inputs(
    &self,
    epoch: &PermissionSnapshotGeneration,
  ) -> anyhow::Result<Arc<PermissionSnapshotInputs>> {
    Ok(
      epoch
        .inputs
        .get_or_try_init(|| async {
          load_snapshot_inputs().await.map(Arc::new)
        })
        .await?
        .clone(),
    )
  }
}
```

The generation-scoped `OnceCell` loads all authoritative Permission rows and
all readable resource base permissions once per Core/generation, not once per
user. `from_inputs` still filters the shared rows by current user and current
group membership, so sharing cannot transfer one user's grants to another.
The single aggregate preserves the seven-operation cold budget and prevents an
`O(users * resources)` rebuild stampede.

Import `AggregateOptions` and `Bson`. The 20,000-document first batch is paired
with the 8 MiB profiler response gate: accepted fixtures fit in one cursor
batch, so the cold `<= 7` count may continue to include getMore and fails if a
larger input silently adds cursor round trips.

Implement `build_snapshot` with these inputs:

```rust
async fn build_snapshot(
  provider: &PermissionSnapshotProvider,
  session_user: &User,
  epoch: &PermissionSnapshotGeneration,
) -> anyhow::Result<UserPermissionSnapshot> {
  let generation = epoch.generation;
  let user = get_user(&session_user.id).await?;
  let transparent_mode = core_config().transparent_mode;
  if !user.enabled || user.admin || transparent_mode {
    return Ok(UserPermissionSnapshot {
      generation,
      enabled: user.enabled,
      unrestricted: user.admin || transparent_mode,
      scopes: HashMap::new(),
    });
  }
  let groups = get_user_user_groups(&user.id).await?;
  let inputs = provider.inputs(epoch).await?;
  Ok(UserPermissionSnapshot::from_inputs(
    &user,
    &groups,
    &inputs,
    generation,
    transparent_mode,
  ))
}
```

Implement `from_inputs` as this pure function:

```rust
impl UserPermissionSnapshot {
  fn from_inputs(
    user: &User,
    groups: &[UserGroup],
    inputs: &PermissionSnapshotInputs,
    generation: i64,
    transparent_mode: bool,
  ) -> Self {
    let mut scopes = HashMap::new();
    macro_rules! insert_scope {
      ($variant:ident) => {{
        let variant = ResourceTargetVariant::$variant;
        let mut base = if user.admin {
          PermissionLevel::Write.all()
        } else if transparent_mode {
          PermissionLevel::Read.into()
        } else {
          PermissionLevel::None.into()
        };
        if let Some(permission) = user.all.get(&variant) {
          base.elevate(permission);
        }
        for group in groups {
          if let Some(permission) = group.all.get(&variant) {
            base.elevate(permission);
          }
        }
        if base.fulfills(&PermissionLevel::Read.into()) {
          scopes.insert(variant, ResourceReadScope::All);
        } else {
          let mut ids = inputs
            .base_ids
            .get(&variant)
            .cloned()
            .unwrap_or_default();
          if let Some(direct) = inputs
            .user_ids
            .get(&user.id)
            .and_then(|by_variant| by_variant.get(&variant))
          {
            ids.extend(direct.iter().cloned());
          }
          for group in groups {
            if let Some(inherited) = inputs
              .group_ids
              .get(&group.id)
              .and_then(|by_variant| by_variant.get(&variant))
            {
              ids.extend(inherited.iter().cloned());
            }
          }
          scopes.insert(variant, ResourceReadScope::Ids(ids));
        }
      }};
    }
    insert_scope!(Swarm);
    insert_scope!(Server);
    insert_scope!(Stack);
    insert_scope!(Deployment);
    insert_scope!(Build);
    insert_scope!(Repo);
    insert_scope!(Procedure);
    insert_scope!(Action);
    insert_scope!(ResourceSync);
    insert_scope!(Builder);
    insert_scope!(Alerter);
    Self {
      generation,
      enabled: user.enabled,
      unrestricted: user.admin || transparent_mode,
      scopes,
    }
  }
}
```

Add these pure tests below the two Step 1 tests:

```rust
fn server_base_permission() -> ResourceBasePermission {
  ResourceBasePermission {
    resource_type: ResourceTargetVariant::Server,
    resource_id: "server-1".into(),
    base_permission: PermissionLevel::Read.into(),
  }
}

fn grant(target: UserTarget) -> Permission {
  Permission {
    id: String::new(),
    user_target: target,
    resource_target: ResourceTarget::Server("server-1".into()),
    level: PermissionLevel::Read,
    specific: Default::default(),
  }
}

#[test]
fn direct_grant_and_revocation_change_scope() {
  let mut user = User::default();
  user.id = "user-1".into();
  user.enabled = true;
  let direct = grant(UserTarget::User(user.id.clone()));
  let granted_inputs = PermissionSnapshotInputs::from_rows(
    vec![direct],
    vec![],
  );
  let granted = UserPermissionSnapshot::from_inputs(
    &user,
    &[],
    &granted_inputs,
    1,
    false,
  );
  assert!(granted.can_read(&ResourceTarget::Server(
    "server-1".into(),
  )));
  let revoked = UserPermissionSnapshot::from_inputs(
    &user,
    &[],
    &PermissionSnapshotInputs::from_rows(vec![], vec![]),
    2,
    false,
  );
  assert!(!revoked.can_read(&ResourceTarget::Server(
    "server-1".into(),
  )));
}

#[test]
fn group_grant_requires_current_membership() {
  let mut user = User::default();
  user.id = "user-1".into();
  user.enabled = true;
  let mut group = UserGroup::default();
  group.id = "group-1".into();
  group.users.push(user.id.clone());
  let inherited = grant(UserTarget::UserGroup(group.id.clone()));
  let inputs = PermissionSnapshotInputs::from_rows(
    vec![inherited],
    vec![],
  );
  let member = UserPermissionSnapshot::from_inputs(
    &user,
    &[group.clone()],
    &inputs,
    1,
    false,
  );
  assert!(member.can_read(&ResourceTarget::Server(
    "server-1".into(),
  )));
  let removed = UserPermissionSnapshot::from_inputs(
    &user,
    &[],
    &inputs,
    2,
    false,
  );
  assert!(!removed.can_read(&ResourceTarget::Server(
    "server-1".into(),
  )));
}

#[test]
fn authoritative_resource_base_permission_adds_scope() {
  let mut user = User::default();
  user.id = "user-1".into();
  user.enabled = true;
  let base = server_base_permission();
  let inputs = PermissionSnapshotInputs::from_rows(
    vec![],
    vec![base],
  );
  let snapshot = UserPermissionSnapshot::from_inputs(
    &user,
    &[],
    &inputs,
    1,
    false,
  );
  assert!(snapshot.can_read(&ResourceTarget::Server(
    "server-1".into(),
  )));
}
```

The one generation-scoped aggregate loads authoritative Permission and
resource-base rows. `PermissionSnapshotInputs::from_rows` indexes them once by
user/group and resource variant; a per-user snapshot touches only its direct
and current-group buckets rather than rescanning global rows eleven times.
Explicit permission IDs never depend on process-local resource inventory, so a
grant created on Core A is immediately usable by a cold snapshot on Core B.
The rollout and recovery invariant scan must reject orphan permission rows
before snapshot reads are enabled.

- [ ] **Step 5: Implement double-read linearization and authoritative fallback**

Add:

```rust
impl PermissionSnapshotProvider {
  async fn snapshot(
    &self,
    user: &User,
    generation: i64,
  ) -> anyhow::Result<Arc<UserPermissionSnapshot>> {
    let epoch = self.generation(generation).await;
    let now_ms = now_unix_ms();
    let entry = if let Some(entry) = epoch
      .snapshots
      .read()
      .await
      .get(&user.id)
      .cloned()
    {
      entry.last_access_ms.store(now_ms, Ordering::Release);
      entry
    } else {
      let mut snapshots = epoch.snapshots.write().await;
      if let Some(entry) = snapshots.get(&user.id).cloned() {
        entry.last_access_ms.store(now_ms, Ordering::Release);
        entry
      } else {
        prune_user_snapshots(&mut snapshots, now_ms);
        let entry = Arc::new(CachedUserSnapshot::new(now_ms));
        snapshots.insert(user.id.clone(), entry.clone());
        entry
      }
    };
    Ok(
      entry
        .cell
        .get_or_try_init(|| async {
          build_snapshot(self, user, &epoch)
            .await
            .map(Arc::new)
        })
        .await?
        .clone(),
    )
  }

  pub async fn can_read_target(
    &self,
    user: &User,
    target: &ResourceTarget,
  ) -> anyhow::Result<bool> {
    for _ in 0..3 {
      let before = match permission_cache_state().await {
        Ok(state) => state,
        Err(_) => {
          return authoritative_can_read_for_session(user, target)
            .await;
        }
      };
      let allowed = if before.cache_reads_enabled
        && !before.mutation_in_progress
      {
        match self.snapshot(user, before.generation).await {
          Ok(snapshot) => snapshot.can_read(target),
          Err(_) => {
            authoritative_can_read_for_session(user, target)
              .await?
          }
        }
      } else {
        authoritative_can_read_for_session(user, target).await?
      };
      let after = match permission_cache_state().await {
        Ok(state) => state,
        Err(_) => {
          return authoritative_can_read_for_session(user, target)
            .await;
        }
      };
      if before.generation == after.generation
        && before.cache_reads_enabled == after.cache_reads_enabled
        && before.mutation_in_progress
          == after.mutation_in_progress
      {
        return Ok(allowed);
      }
    }
    Ok(false)
  }
}
```

The warm path takes a shared read lock and one hash lookup; it never scans all
cached users and never takes an exclusive lock for an existing user. A
generation change swaps one `Arc<PermissionSnapshotGeneration>` in O(1); old
in-flight readers finish on the old Arc, and an older state read cannot replace
a newer generation. Each generation retains at most 4,096 user cells and drops
cells idle for more than fifteen minutes when a new user is inserted. Eviction
only frees memory: a later request rebuilds from the same authoritative
generation and still passes the before/after guard reads, so TTL and capacity
are never authorization correctness mechanisms.

Add `generation_swap_is_constant_with_many_cached_users`: populate 10,000
synthetic entries through the pure pruning helper, assert only the 4,096 newest
remain, request generation 2, and assert the new epoch is empty while an old
in-flight Arc remains readable. Add
`many_users_hit_shared_read_fast_path`: prepopulate 1,000 cells, issue 10,000
concurrent cell lookups under a two-second test timeout, and assert no write
path counter increments. Add `idle_user_snapshot_is_evicted_without_changing_permission`:
inject timestamps into `prune_user_snapshots`, evict one cell past fifteen
minutes, rebuild it at the same generation, and assert the same decision plus
two successful guard reads. Run all three tests in release mode as the
warm-lock and bounded-memory gate.

Plan 2 does not read the generation independently. Its ordered fan-out hub
shares one completed `can_read_target` result only among simultaneous
connections for the same user and the same event. A later event always calls
the provider again, so the provider's own before/after state reads remain the
only authorization linearization mechanism.

The second successful state read is the authorization linearization point. Three unstable attempts fail closed. A missing/unreadable generation bypasses snapshots and uses authoritative reads.

- [ ] **Step 6: Replace the eleven-type query fan-out**

Rename the current free function in `permission.rs` to
`authoritative_user_resource_target_query`, then add this provider method:

```rust
impl PermissionSnapshotProvider {
  pub async fn user_resource_target_query(
    &self,
    user: &User,
    incoming_query: Option<Document>,
  ) -> anyhow::Result<Option<Document>> {
    for _ in 0..3 {
      let before = match permission_cache_state().await {
        Ok(state) => state,
        Err(_) => {
          return authoritative_query_for_session(
            user,
            incoming_query,
          )
          .await;
        }
      };
      let query = if before.cache_reads_enabled
        && !before.mutation_in_progress
      {
        match self.snapshot(user, before.generation).await {
          Ok(snapshot) if !snapshot.enabled => {
            Some(deny_query())
          }
          Ok(snapshot) if snapshot.unrestricted => {
            incoming_query.clone()
          }
          Ok(snapshot) => {
            combine_query(
              snapshot_access_query(
                &snapshot,
                incoming_query.as_ref(),
              ),
              incoming_query.clone(),
            )
          }
          Err(_) => authoritative_query_for_session(
            user,
            incoming_query.clone(),
          )
          .await?,
        }
      } else {
        authoritative_query_for_session(
          user,
          incoming_query.clone(),
        )
        .await?
      };
      let after = match permission_cache_state().await {
        Ok(state) => state,
        Err(_) => {
          return authoritative_query_for_session(
            user,
            incoming_query,
          )
          .await;
        }
      };
      if before.generation == after.generation
        && before.cache_reads_enabled == after.cache_reads_enabled
        && before.mutation_in_progress
          == after.mutation_in_progress
      {
        return Ok(query);
      }
    }
    Ok(Some(deny_query()))
  }
}
```

This uses one stable `UserPermissionSnapshot`; an exact direct
`target.type` query creates one type clause, while arbitrary filters create
the eleven clauses from the same snapshot. Disabled, missing, guarded, or
failed snapshot state uses the authoritative implementation. Three unstable
generation attempts return a filter that matches nothing.

Keep `System` excluded for enabled non-admin users outside transparent mode.
Unit-test exact Server targeting, an enabled current admin reading `System`, a
disabled current user receiving the deny query, unrestricted type, empty ID
set, arbitrary `$or` fallback, and generation change between reads. Never use
the session copy of `admin`, `enabled`, or `all`; the generation snapshot and
authoritative fallback both reload the current `User` document.

- [ ] **Step 7: Route List and Get endpoints through the provider**

At `api/read/update.rs:36` and `api/read/alert.rs:33` call:

```rust
let query = permission_snapshot_provider()
  .user_resource_target_query(user, self.query)
  .await?;
```

For `GetUpdate` and `GetAlert` replace `check_user_target_access` with:

```rust
if !permission_snapshot_provider()
  .can_read_target(user, &update.target)
  .await?
{
  return Err(anyhow::anyhow!(
    "user does not have read permission on update target"
  )
  .into());
}
```

Use `alert.target` and the alert-specific error in `GetAlert`.
Delete both existing `if user.admin || core_config().transparent_mode { return
Ok(...) }` branches before this call, and remove the now-unused
`core_config`, `PermissionLevel`, and `check_user_target_access` imports from
the two read modules. Every caller—including an already-open admin session—must
enter the provider so it reloads the current User and observes demotion or
disablement.

- [ ] **Step 8: Add a generation-forced cold-snapshot profiler**

Extend the Task 1 ignored resolver test with
`P1_PROFILE_MODE=cold`. After loading the fixture User but before opening the
Mongo server-time window, it executes the same CAS as the operator `enable`
action to advance the generation and keep reads enabled. It does no warmup in
cold mode. All profiler filtering and unconditional cleanup remain identical
to warm mode.

Create `scripts/performance/profile-cold-permission-snapshot.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

: "${MONGODB_URI:?set MONGODB_URI selecting the fixture database}"
: "${P1_DATABASE_NAME:?set P1_DATABASE_NAME to the same p1_* database}"
: "${FIXTURE_SIZE:?set FIXTURE_SIZE to 1, 100, or 1000}"
: "${FIXTURE_MANIFEST:?set FIXTURE_MANIFEST to the fixture JSON}"
: "${P1_PROFILE_USER_ID:?set P1_PROFILE_USER_ID}"

case "$P1_DATABASE_NAME" in
  p1_*) ;;
  *) echo "P1_DATABASE_NAME must start with p1_" >&2; exit 2 ;;
esac
actual_database=$(mongosh "$MONGODB_URI" --quiet --eval 'print(db.getName())' | tail -1)
test "$actual_database" = "$P1_DATABASE_NAME" || {
  echo "mongosh selected $actual_database, expected $P1_DATABASE_NAME" >&2
  exit 2
}

case "${ENFORCE_BUDGET:-0}" in
  0|1) ;;
  *) echo "ENFORCE_BUDGET must be 0 or 1" >&2; exit 2 ;;
esac

artifact="core-data-path-cold-$(git rev-parse --short HEAD)-${FIXTURE_SIZE}.json"
work_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$work_dir"
}
trap cleanup EXIT HUP INT TERM

mongosh "$MONGODB_URI" --quiet \
  scripts/performance/validate-core-data-fixture.js \
  > "$work_dir/fixture.json"
manifest_user_id=$(jq -er '.user_id' "$work_dir/fixture.json")
test "$manifest_user_id" = "$P1_PROFILE_USER_ID" || {
  echo "P1_PROFILE_USER_ID does not match fixture manifest" >&2
  exit 2
}

actual_fixture_size=$(
  mongosh "$MONGODB_URI" --quiet --eval \
    'print(db.Server.countDocuments({}))' | tail -1
)
expected_server_count=$((FIXTURE_SIZE + 1))
test "$actual_fixture_size" = "$expected_server_count" || {
  echo "Server fixture count is $actual_fixture_size, expected $expected_server_count including denied sentinel" >&2
  exit 2
}

profile_cold() {
  local endpoint="$1"
  local output="$2"
  local app_name="p1-resolver-cold-$endpoint-$FIXTURE_SIZE-$$"
  P1_ALLOW_MONGO_PROFILER=1 \
  P1_PROFILE_MODE=cold \
  P1_PROFILE_ENDPOINT="$endpoint" \
  P1_PROFILE_USER_ID="$P1_PROFILE_USER_ID" \
  P1_PROFILE_ARTIFACT="$output" \
  KOMODO_DATABASE_URI="$MONGODB_URI" \
  KOMODO_DATABASE_DB_NAME="$P1_DATABASE_NAME" \
  KOMODO_DATABASE_APP_NAME="$app_name" \
    cargo test -p komodo_core --release \
      api::read::staging::profile_list_endpoint_from_env \
      -- --ignored --exact --nocapture --test-threads=1
}

profile_cold list_updates "$work_dir/list-updates.json"
profile_cold list_alerts "$work_dir/list-alerts.json"

jq -n \
  --arg fixture_size "$FIXTURE_SIZE" \
  --argjson enforce "${ENFORCE_BUDGET:-0}" \
  --slurpfile updates "$work_dir/list-updates.json" \
  --slurpfile alerts "$work_dir/list-alerts.json" \
  '($updates[0].total_commands) as $updates_after_auth |
   ($alerts[0].total_commands) as $alerts_after_auth |
   ($updates[0].docs_examined) as $updates_docs |
   ($alerts[0].docs_examined) as $alerts_docs |
   ($updates[0].response_bytes) as $updates_bytes |
   ($alerts[0].response_bytes) as $alerts_bytes |
   if $updates_after_auth > 7 or $alerts_after_auth > 7 then
     error("cold permission snapshot exceeded seven-command budget")
   elif $enforce == 1 and
        ($updates_docs > 20000 or $alerts_docs > 20000 or
         $updates_bytes > 8388608 or $alerts_bytes > 8388608) then
     error("cold permission snapshot exceeded work or response-byte budget")
   else {
     fixture_size: ($fixture_size | tonumber),
     list_updates_commands: $updates[0],
     list_alerts_commands: $alerts[0],
     list_updates_after_auth: $updates_after_auth,
     list_alerts_after_auth: $alerts_after_auth,
     list_updates_docs_examined_after_auth: $updates_docs,
     list_alerts_docs_examined_after_auth: $alerts_docs,
     list_updates_response_bytes_after_auth: $updates_bytes,
     list_alerts_response_bytes_after_auth: $alerts_bytes
   } end' > "$artifact"

jq . "$artifact"
```

This wrapper launches a fresh direct-resolver process per endpoint; no Core
background loop exists. In `cold` mode the staging test advances the generation
before its server-time window, then invokes the real resolver once. The control
write, User fixture lookup, profiler setup, and artifact query are outside the
window, so the totals require no baseline subtraction.

- [ ] **Step 9: Run permission tests and warm/cold command budgets**

Run:

```bash
rtk cargo test -p komodo_core permission::snapshot
rtk cargo test -p komodo_core
rtk cargo test -p komodo_core --release permission::snapshot::tests::generation_swap_is_constant_with_many_cached_users -- --exact
rtk cargo test -p komodo_core --release permission::snapshot::tests::many_users_hit_shared_read_fast_path -- --exact
rtk cargo test -p komodo_core --release permission::snapshot::tests::idle_user_snapshot_is_evicted_without_changing_permission -- --exact
rtk chmod +x scripts/performance/profile-cold-permission-snapshot.sh
rtk bash -n scripts/performance/profile-cold-permission-snapshot.sh
```

Expected: PASS.

With snapshot reads still disabled, run the profiler and confirm authoritative behavior matches pre-change. Then enable only on isolated staging:

```bash
rtk env PERMISSION_CACHE_ACTION=enable mongosh "$MONGODB_URI_FIXTURE_1" --quiet scripts/performance/permission-cache-control.js
rtk env PERMISSION_CACHE_ACTION=enable mongosh "$MONGODB_URI_FIXTURE_100" --quiet scripts/performance/permission-cache-control.js
rtk env PERMISSION_CACHE_ACTION=enable mongosh "$MONGODB_URI_FIXTURE_1000" --quiet scripts/performance/permission-cache-control.js
rtk env FIXTURE_SIZE=1 FIXTURE_MANIFEST="$FIXTURE_MANIFEST_1" P1_DATABASE_NAME="$P1_DATABASE_NAME_1" P1_PROFILE_USER_ID="$P1_PROFILE_USER_ID_1" ENFORCE_BUDGET=1 MONGODB_URI="$MONGODB_URI_FIXTURE_1" KOMODO_ADDRESS="$KOMODO_ADDRESS_FIXTURE_1" scripts/performance/profile-core-data-paths.sh
rtk env FIXTURE_SIZE=100 FIXTURE_MANIFEST="$FIXTURE_MANIFEST_100" P1_DATABASE_NAME="$P1_DATABASE_NAME_100" P1_PROFILE_USER_ID="$P1_PROFILE_USER_ID_100" ENFORCE_BUDGET=1 MONGODB_URI="$MONGODB_URI_FIXTURE_100" KOMODO_ADDRESS="$KOMODO_ADDRESS_FIXTURE_100" scripts/performance/profile-core-data-paths.sh
rtk env FIXTURE_SIZE=1000 FIXTURE_MANIFEST="$FIXTURE_MANIFEST_1000" P1_DATABASE_NAME="$P1_DATABASE_NAME_1000" P1_PROFILE_USER_ID="$P1_PROFILE_USER_ID_1000" ENFORCE_BUDGET=1 MONGODB_URI="$MONGODB_URI_FIXTURE_1000" KOMODO_ADDRESS="$KOMODO_ADDRESS_FIXTURE_1000" scripts/performance/profile-core-data-paths.sh
rtk env ENFORCE_BUDGET=1 scripts/performance/profile-core-resolver-commands.sh
rtk env FIXTURE_SIZE=1 FIXTURE_MANIFEST="$FIXTURE_MANIFEST_1" P1_PROFILE_USER_ID="$P1_PROFILE_USER_ID_1" P1_DATABASE_NAME="$P1_DATABASE_NAME_1" ENFORCE_BUDGET=1 MONGODB_URI="$MONGODB_URI_FIXTURE_1" scripts/performance/profile-cold-permission-snapshot.sh
rtk env FIXTURE_SIZE=100 FIXTURE_MANIFEST="$FIXTURE_MANIFEST_100" P1_PROFILE_USER_ID="$P1_PROFILE_USER_ID_100" P1_DATABASE_NAME="$P1_DATABASE_NAME_100" ENFORCE_BUDGET=1 MONGODB_URI="$MONGODB_URI_FIXTURE_100" scripts/performance/profile-cold-permission-snapshot.sh
rtk env FIXTURE_SIZE=1000 FIXTURE_MANIFEST="$FIXTURE_MANIFEST_1000" P1_PROFILE_USER_ID="$P1_PROFILE_USER_ID_1000" P1_DATABASE_NAME="$P1_DATABASE_NAME_1000" ENFORCE_BUDGET=1 MONGODB_URI="$MONGODB_URI_FIXTURE_1000" scripts/performance/profile-cold-permission-snapshot.sh
rtk proxy sh -c 'set -eu; sha=$(rtk git rev-parse --short HEAD); rtk jq -e -s '\''([.[].list_updates_after_auth] | unique | length) == 1 and ([.[].list_alerts_after_auth] | unique | length) == 1'\'' "core-data-path-cold-$sha-1.json" "core-data-path-cold-$sha-100.json" "core-data-path-cold-$sha-1000.json"'
```

Expected: the warm endpoint command count is <= 4; every cold sample is <= 7;
the final equality check proves both cold counts are constant at all fixture
sizes.

- [ ] **Step 10: Commit permission snapshots**

```bash
rtk git add bin/core/src/permission.rs bin/core/src/permission/snapshot.rs bin/core/src/api/read/update.rs bin/core/src/api/read/alert.rs bin/core/src/api/read/staging.rs scripts/performance/profile-core-resolver-commands.sh scripts/performance/profile-cold-permission-snapshot.sh
rtk git commit -m "perf: cache generation-scoped permissions"
```

Expected: the commit exposes
`PermissionSnapshotProvider::can_read_target` as the only Plan 2 authorization
contract; its internal post-decision generation recheck is documented and
tested here.

### Task 14: Prove cross-Core revocation, recovery, rollout, and rollback

**Files:**
- Verify: `bin/core/Cargo.toml`
- Verify: `bin/core/src/permission/mutation.rs`
- Verify: `bin/core/src/api/write/{permissions,user_group,user}.rs`
- Verify: `bin/core/src/helpers/mod.rs`
- Verify: `bin/core/src/resource/mod.rs`
- Verify: `bin/core/src/startup.rs`
- Create: `scripts/performance/verify-permission-cache-cross-core.sh`
- Create: `scripts/performance/verify-permission-mutation-matrix.sh`
- Create: `scripts/performance/verify-permission-finalize-failure.sh`
- Modify: `docs/performance/core-data-path-budgets.md`

- [ ] **Step 1: Create the two-Core revocation test**

Create `scripts/performance/verify-permission-cache-cross-core.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

for name in CORE_A CORE_B ADMIN_API_KEY ADMIN_API_SECRET USER_API_KEY USER_API_SECRET USER_ID RESOURCE_ID UPDATE_ID MONGODB_URI; do
  test -n "${!name:-}" || {
    echo "missing $name" >&2
    exit 2
  }
done

work_dir=$(mktemp -d)
trap 'rm -rf "$work_dir"' EXIT HUP INT TERM

write_permission() {
  local level="$1"
  curl --fail --silent --show-error \
    --header 'content-type: application/json' \
    --header "x-api-key: $ADMIN_API_KEY" \
    --header "x-api-secret: $ADMIN_API_SECRET" \
    --data "{
      \"type\":\"UpdatePermissionOnTarget\",
      \"params\":{
        \"user_target\":{\"type\":\"User\",\"id\":\"$USER_ID\"},
        \"resource_target\":{\"type\":\"Server\",\"id\":\"$RESOURCE_ID\"},
        \"permission\":{\"level\":\"$level\",\"specific\":[]}
      }
    }" \
    "$CORE_A/write" >/dev/null
}

read_update() {
  local core="$1"
  local label="$2"
  # Curl still exits nonzero for DNS/connect/TLS/transport failures. HTTP
  # errors are captured so their exact status and body can be asserted.
  curl --silent --show-error \
    --connect-timeout 5 --max-time 15 \
    --output "$work_dir/$label.body" \
    --write-out '%{http_code}' \
    --header 'content-type: application/json' \
    --header "x-api-key: $USER_API_KEY" \
    --header "x-api-secret: $USER_API_SECRET" \
    --data "{
      \"type\":\"GetUpdate\",
      \"params\":{\"id\":\"$UPDATE_ID\"}
    }" \
    "$core/read" > "$work_dir/$label.status"
}

assert_core_live() {
  local core="$1"
  curl --fail --silent --show-error \
    --connect-timeout 5 --max-time 15 \
    --header 'content-type: application/json' \
    --header "x-api-key: $USER_API_KEY" \
    --header "x-api-secret: $USER_API_SECRET" \
    --data '{"type":"GetVersion","params":{}}' \
    "$core/read" >/dev/null
}

assert_allowed() {
  local core="$1"
  local label="$2"
  read_update "$core" "$label"
  test "$(cat "$work_dir/$label.status")" = 200
}

assert_denied() {
  local core="$1"
  local label="$2"
  read_update "$core" "$label"
  test "$(cat "$work_dir/$label.status")" = 500
  rg -F 'user does not have read permission on update target' \
    "$work_dir/$label.body"
  assert_core_live "$core"
}

env PERMISSION_CACHE_ACTION=disable \
  mongosh "$MONGODB_URI" --quiet \
  scripts/performance/permission-cache-control.js >/dev/null
write_permission Read

env PERMISSION_CACHE_ACTION=enable \
  mongosh "$MONGODB_URI" --quiet \
  scripts/performance/permission-cache-control.js >/dev/null

assert_allowed "$CORE_A" core-a-granted
assert_allowed "$CORE_B" core-b-granted

write_permission None

assert_denied "$CORE_A" core-a-revoked
assert_denied "$CORE_B" core-b-revoked

env PERMISSION_CACHE_ACTION=disable \
  mongosh "$MONGODB_URI" --quiet \
  scripts/performance/permission-cache-control.js >/dev/null

assert_denied "$CORE_A" core-a-disabled
assert_denied "$CORE_B" core-b-disabled
```

- [ ] **Step 2: Run the cross-Core test**

Run:

```bash
rtk chmod +x scripts/performance/verify-permission-cache-cross-core.sh
rtk bash -n scripts/performance/verify-permission-cache-cross-core.sh
rtk scripts/performance/verify-permission-cache-cross-core.sh
```

Expected: exit 0. A revocation finalized through Core A is denied immediately by both Core A and Core B.

- [ ] **Step 3: Run the unit mutation matrix and the live two-Core proof**

Re-run the `permission::mutation::matrix` module committed by Task 12. Its
injected backend records `acquire → authoritative write(s) → finalize`, can
fail after any write, and is called through the same resolver/helper
`*_guarded_with` functions as production—not through a duplicate mock
implementation. Confirm it still covers:

- direct user permission;
- group base permission;
- user-group membership add/remove/set/everyone;
- UserGroup-to-resource permission row (the approved “resource-group membership” category);
- group deletion;
- user deletion;
- resource creation grant;
- resource deletion;
- admin promotion/demotion, including `System` visibility from an already-open
  session;
- user enable/disable and create-server/create-build capability changes.

Create `scripts/performance/verify-permission-mutation-matrix.sh`:

```sh
#!/usr/bin/env sh
set -eu

cargo test -p komodo_core permission::mutation::matrix -- --nocapture
scripts/performance/audit-permission-writers.sh
cargo test -p komodo_core permission::mutation::tests::failed_permission_write_does_not_finalize_guard -- --exact
cargo test -p komodo_core permission::mutation::tests::wrong_recovery_token_is_rejected -- --exact
cargo test -p komodo_core permission::mutation::tests::successful_recovery_disables_reads_and_advances_generation -- --exact
cargo test -p komodo_core permission::mutation::tests::disable_during_held_mutation_preserves_finalize_token -- --exact
```

Run:

```bash
rtk chmod +x scripts/performance/verify-permission-mutation-matrix.sh
rtk sh -n scripts/performance/verify-permission-mutation-matrix.sh
rtk scripts/performance/verify-permission-mutation-matrix.sh
```

The mutation matrix above is a deterministic single-process unit gate; do not
describe it as a two-Core test. Separately rerun the Step 2 script against two
live Core processes for the real direct grant, revoke, and kill-switch proof.

For the forced-failure case, build only isolated Core A with the Task 12
feature. Core B uses the normal binary. The verification script—not an operator
shell—owns Core A's complete lifecycle:

```bash
rtk cargo build -p komodo_core --release --features p1-permission-finalize-failpoint
```

Create `scripts/performance/verify-permission-finalize-failure.sh`. It requires
the Step 1 Core B address, credentials, IDs, `MONGODB_URI`, explicit
`P1_DATABASE_NAME`, readable `FIXTURE_MANIFEST`, `CORE_A_ENV_FILE`, `CORE_A_PORT`, and
`CORE_A_BINARY=target/release/core`; copies
the exact transport-safe read/deny/liveness helpers from Step 1; uses a
temporary directory with an exit trap; and executes this fixed sequence:

0. Require `P1_DATABASE_NAME` to start with `p1_`, compare it with
   `mongosh "$MONGODB_URI" ... db.getName()`, require
   `test -r "$FIXTURE_MANIFEST"`, pass it as `FIXTURE_MANIFEST` to
   `validate-core-data-fixture.js`,
   and require its `user_id` to equal the API-key user. Set
   `CORE_A_LOG="$work_dir/core-a.log"`, run
   `set -a; . "$CORE_A_ENV_FILE"; set +a` to export the untracked dotenv
   without printing it, then explicitly `export` only the following overrides:
   `KOMODO_DATABASE_URI="$MONGODB_URI"`,
   `KOMODO_DATABASE_DB_NAME="$P1_DATABASE_NAME"`,
   `KOMODO_BIND_IP=127.0.0.1`, `KOMODO_PORT="$CORE_A_PORT"`,
   `KOMODO_HOST="http://127.0.0.1:$CORE_A_PORT"`,
   `KOMODO_P1_ALLOW_PERMISSION_FAILPOINT=1`, and
   `KOMODO_P1_FAIL_PERMISSION_FINALIZE_ONCE=DirectUserPermission`. Launch
   `"$CORE_A_BINARY" >"$CORE_A_LOG" 2>&1 &`, save `CORE_A_PID`, and install a
   trap that sends TERM, waits, and then removes the temporary directory on
   every exit/signal. Poll `GetVersion` through
   `http://127.0.0.1:$CORE_A_PORT` with `curl --fail --connect-timeout 1
   --max-time 2` for at most 60 seconds, checking `kill -0 "$CORE_A_PID"`
   between attempts; fail with the redacted log tail if readiness is not
   reached. All later Core A requests use this exact loopback address.

1. Require Core A's log to contain exactly one
   `permission finalize failpoint armed: DirectUserPermission` marker. Through normal Core
   B, create the initial `Read` grant and enable snapshots. Assert exact HTTP
   200 reads through both Cores.
2. Send the literal `UpdatePermissionOnTarget` level-`None` request through
   Core A with `curl --silent --show-error --connect-timeout 5 --max-time 15`,
   saving body and status. Transport failure is fatal. Require status 500, the
   body marker `generation finalization failed; cache remains bypassed`, and
   exactly one `permission finalize failpoint consumed: DirectUserPermission`
   marker in `CORE_A_LOG`.
3. Read `PermissionCacheState/global` through two independent `mongosh`
   processes, save canonical EJSON, and `cmp` them. With `jq`, require one
   nonempty `mutation_id`, `mutation_in_progress=true`, the expected mutation
   kind, and the same numeric generation. Query the Permission row and prove
   the readable grant is already absent.
4. Use Step 1's exact denied-body plus GetVersion-liveness assertions through
   Core A and Core B. This proves guard-driven authoritative fallback without
   accepting `000`, `401`, a crash, or an unrelated 500.
5. Run `permission-cache-control.js` recovery once with a wrong token and once
   with `generation + 1`; both commands must return nonzero. Then recover with
   the saved token/generation. Require generation to advance exactly once,
   `mutation_in_progress=false`, and `cache_reads_enabled=false`; assert exact
   denial and liveness through both Cores again.

The script leaves reads disabled. Run:

```bash
rtk chmod +x scripts/performance/verify-permission-finalize-failure.sh
rtk bash -n scripts/performance/verify-permission-finalize-failure.sh
rtk scripts/performance/verify-permission-finalize-failure.sh
```

Expected: the unit matrix covers every mutation category and recovery CAS
contract. In the separate feature-gated live test,
`mutation_in_progress=true`, both Cores
bypass snapshots, and neither returns a stale grant. `recover` refuses an
incorrect token or generation; the correct token/generation plus a valid
reference scan clears the guard, increments generation, and leaves reads
disabled.

- [ ] **Step 4: Record the rolling rollout and rollback**

Append:

```markdown
## Permission snapshot rollout

1. Deploy PermissionCacheState initialization and guarded mutation code to every Core with cache_reads_enabled=false.
2. Verify every Core SHA, run the single-process mutation matrix and writer
   audit, then run the direct grant/revoke and kill-switch script against two
   live Core processes while cache reads are disabled.
3. Enable with permission-cache-control.js; this increments generation.
4. Run warm/cold command budgets and revocation tests.
5. Plan 2 may consume PermissionSnapshotProvider only after this gate merges.

## Permission snapshot rollback

1. Disable with permission-cache-control.js before starting any old Core.
2. Verify cache_reads_enabled=false from two independent Mongo reads.
3. Drain new Core instances, then deploy the old version.
4. Keep the state document; old versions ignore it.
5. Never clear mutation_in_progress without the recovery invariant scan and matching CAS token.
```

- [ ] **Step 5: Run final static and test gates**

Run:

```bash
rtk proxy sh -c 'if rtk rg -n "all_resources_cache\\(\\)\\.(load|store)\\(|AllResourcesById::load\\(|refresh_all_resources_cache" bin/core/src; then exit 1; fi'
rtk scripts/performance/audit-resource-cache-writers.sh
rtk scripts/performance/audit-permission-writers.sh
rtk cargo fmt --all -- --check
rtk cargo test -p database
rtk cargo test -p komodo_core
rtk git diff --check
```

Expected: the raw cache search has no matches; format, database tests, Core tests, and diff check exit 0.

- [ ] **Step 6: Commit Merge Gate A documentation and cross-Core test**

```bash
rtk git add scripts/performance/verify-permission-cache-cross-core.sh scripts/performance/verify-permission-mutation-matrix.sh scripts/performance/verify-permission-finalize-failure.sh docs/performance/core-data-path-budgets.md
rtk git commit -m "test: verify cross-core permission revocation"
```

Expected: checkpoint 4 is independently deployable, cache reads remain
operator-controlled, and Plan 2 has one reviewed provider API:
`can_read_target`.

## Final review checklist

- [ ] Every one of the four assigned P1 findings has a failing test, measured baseline, implementation checkpoint, numeric exit gate, and rollback.
- [ ] Production `getIndexes()` output was reviewed before repository index creation; equivalent manual indexes were reused.
- [ ] Monitoring/state logical starts are constant at 1/100/1,000; getMore, examined rows, bytes, and milliseconds are separately captured and bounded.
- [ ] No resource mutation waits for an eleven-type reload; every affected type is published/dirty, dirty reads singleflight, and repair completes within five seconds.
- [ ] Every effective-permission mutation category passes through one CAS guard, including user admin/base changes, sync resolver paths, and deletion cleanup.
- [ ] Missing state, disabled cache, a set mutation guard, unstable generation, and failed finalization all bypass snapshots or fail closed.
- [ ] Snapshots and authoritative fallbacks reload the current `User`; a stale session cannot preserve admin access or keep a disabled user authorized.
- [ ] The executable mutation matrix covers grant/inheritance/revocation/membership/failure/recovery; real direct grant/revoke and kill switch pass across two Core processes.
- [ ] Plan 2 calls only `can_read_target(&User, &ResourceTarget)`, once per user/event in its ordered hub; it never caches that decision across events.
- [ ] Each checkpoint is committed separately and the PR base/head is verified as `intezya/komodo`.

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-10-komodo-core-data-path-performance.md`. Execute with `superpowers:subagent-driven-development` for fresh task agents and two-stage review, or `superpowers:executing-plans` for checkpoint batches in one session.
