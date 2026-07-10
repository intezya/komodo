# Komodo UI Startup and Background Traffic Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make an authenticated Komodo page download only the code needed by the current route or opened feature, and reduce the synchronized idle-dashboard endpoint families changed by this plan by at least 80% without weakening correctness polling, disconnected, reconnect, missed-event, or old-Core behavior.

**Architecture:** Keep resource names, paths, icons, list-query helpers, and event routing in a lightweight static layer; first patch the locked `mogh_ui@0.6.1` package so its root entry is side-effect free and Monaco is available only through `mogh_ui/monaco`, then resolve each resource implementation and Monaco, Recharts, xterm, Prettier, and editor typings through explicit lazy boundaries. Remove closed-feature observers and duplicate responsive dashboard subtrees before changing freshness semantics. After Runtime Plan 2 Merge Gate B adds optional `stream_epoch` and `sequence`, place a tested stream coordinator in front of update application: login, reconnect, epoch changes, and gaps await an active-query refetch barrier; only a successful barrier enables WebSocket-first cache updates and 60-second safety polling, while old Core, disconnected, synchronizing, and degraded states retain the existing faster fallback cadences.

**Tech Stack:** React 19, TypeScript 6, Vite 8 manifest output, TanStack Query 5, Jotai, Mantine Spotlight, Komodo TypeScript client, Node test runner through `tsx`, browser DevTools HAR and React Profiler.

---

## Scope, ownership, and checkpoint map

This umbrella plan is implemented as three ordered, independently reviewable PR checkpoints:

1. **Current-route chunks and lazy heavy features** — Tasks 1–4, branch `ui-startup-chunks`. This checkpoint is independent of Runtime Plan 2.
2. **Closed-feature traffic and dashboard reuse** — Tasks 5–8, branch `ui-background-traffic`, created from `main` only after checkpoint 1 merges. It keeps current fallback polling intervals.
3. **Sequenced WebSocket synchronization** — Tasks 9–13, branch `ui-update-stream-sync`, created from `main` only after Runtime Plan 2 Merge Gate B and checkpoint 2 merge.

Plan 3 owns all frontend resource loading, query policy, and the checkpoint-3 edit of `ui/src/lib/socket.tsx`. Runtime Plan 2 owns backend event creation, sequence assignment, WebSocket delivery authorization, and compatibility of the event payload. Do not edit Rust event producers in this plan. Do not add P2/P3 UI cleanup while touching these files.

## Confirmed repository facts and live assumptions

Confirmed by source inspection at `b6917bb7` and the 2026-07-10 production Docker build:

- `ui/src/main.tsx` statically imports and calls `initMonaco`; `ui/src/monaco.ts` immediately starts ten `/client/*`, `/index.d.ts`, and `/deno.d.ts` typing requests.
- `ui/src/lib/socket.tsx` imports `ResourceComponents`, and `ui/src/resources/index.ts` statically imports all eleven resource implementations. Those implementations reach Recharts through server stats and xterm through terminal sections. Named `MonacoEditor` and `MonacoDiffEditor` imports from `mogh_ui` also pull Monaco and Prettier into the initial graph.
- The measured Docker build emitted 7.172 MB of initial minified JavaScript and 1.923 MB gzip: the 3,543,873-byte application entry was 996.19 kB gzip and the 3,627,846-byte preloaded `editor.api2` asset was 927.03 kB gzip. The implementation must rerun and freeze this baseline on its own starting commit rather than treating these rounded values as its only evidence.
- `ui/src/app/topbar/index.tsx` mounts two `OmniSearch` trees. TanStack Query shares their keys, so the audit estimate counts thirteen distinct 15-second keys, not twenty-six HTTP requests per interval.
- The audit values 76, 130, and 338 requests/minute are static cadence estimates, not browser measurements: 76 is thirteen Omni keys at 15 seconds plus alerts at 3 seconds and user/version at 30 seconds; 130 adds nine dashboard summaries at 10 seconds; 338 additionally assumes eight Stack and eight Deployment compact cards polling action state at 5 seconds plus eight Stack full-detail queries at 30 seconds.
- `ui/src/pages/dashboard/recents.tsx` renders the same `children` element into both mobile and desktop containers, and compact update badges invoke action-state and Stack full-detail hooks even though their badge state is already in list items.
- `ui/package.json` has no UI test runner. Component and browser behavior therefore use builds, HARs, the browser, and React Profiler; pure sequence/barrier logic gets deterministic Node tests without introducing a DOM test framework.
- The checked-in `client/core/ts/src/types.ts` currently has no `stream_epoch` or `sequence` fields on `UpdateListItem`. Checkpoint 3 must stop if Merge Gate B has not generated both as optional fields.

Production assumptions that must be recorded, not guessed:

- The authenticated QA/Core host, browser version, network conditions, resource counts, dashboard preferences, and whether the session is connected to one or several Core replicas.
- Actual 60-second HTTP counts in the four connected/disconnected generic-page/dashboard workloads below.
- Whether deployment compression/CDN transfer sizes differ materially from local gzip sizes. The local 900,000-byte gzip gate remains deterministic; the browser HAR is supporting delivery evidence.
- Whether permission-filtered delivery causes visible sequence gaps. The UI must handle every observed gap conservatively with the specified barrier; changing backend sequence semantics belongs to Runtime Plan 2.

## Numeric acceptance budgets

- Runner: the same laptop/browser profile and the same authenticated QA/Core target for pre/post browser samples; Docker builds use the same Docker context, architecture, and cold/warm state recorded in the evidence file.
- Bundle samples: three clean production builds before checkpoint 1 and three after it. Record the median shell-plus-dashboard static graph gzip size. Post-change median must be below **900,000 bytes**, and no initial static asset may contain Monaco, Recharts, xterm, or Prettier markers.
- Traffic samples: three 60-second captures for each of `/profile` with WebSocket connected, `/profile` with only `*/ws/update` blocked, `/` with WebSocket connected, and `/` with only `*/ws/update` blocked. Clear Network after the page settles; use the median request count for each workload.
- Checkpoint 2 must not increase either disconnected median by more than **5% or one request, whichever is larger**, because it retains current fallback cadences. It must reduce closed-Omni traffic to zero and compact-card action/full-detail traffic to zero.
- Checkpoint 3 synchronized profile and idle-dashboard medians for the endpoint families explicitly changed by this plan must each be at most **20% of their matching checkpoint-1 owned-family median**, rounded down only after multiplying. The total dashboard median must separately be no greater than `checkpoint-1 untouched median + floor(checkpoint-1 owned median * 0.2)`; correctness polling is never suppressed merely to satisfy the percentage. Disconnected and old-Core total medians retain the checkpoint-2 fallback medians within **5% or one request**.
- Before editor activation, typing-request count must be **0**. First editor activation must request the existing ten typing assets successfully; closing and reopening the editor must add **0 network-transfer bytes** for those assets with the browser cache enabled.
- React Profiler must show one mounted Spotlight root, one `RecentRow` per nonempty resource type, and one `RecentCard` per rendered list item. A breakpoint change must not double those mounts.

### Task 1: Add deterministic bundle and browser-network measurement before behavior changes

**Files:**
- Create: `ui/scripts/report-initial-bundle.mjs`
- Create: `ui/scripts/network-endpoint-families.mjs`
- Create: `ui/scripts/summarize-network-har.mjs`
- Create: `ui/scripts/summarize-network-har.test.mjs`
- Create: `ui/scripts/check-network-gates.mjs`
- Create: `docs/superpowers/evidence/2026-07-10-komodo-ui-startup-background-traffic.md`
- Create: `docs/superpowers/evidence/2026-07-10-komodo-ui-network-gates.json`
- Modify: `ui/vite.config.ts`
- Modify: `ui/package.json`

- [ ] **Step 1: Enable a production manifest and add measurement commands**

Add `build: { manifest: true }` to `defineConfig` in `ui/vite.config.ts`. Add these scripts to `ui/package.json` without changing dependency versions:

```json
"analyze:bundle": "node scripts/report-initial-bundle.mjs",
"analyze:har": "node scripts/summarize-network-har.mjs",
"analyze:har-gates": "node scripts/check-network-gates.mjs",
"test:har": "node --test scripts/summarize-network-har.test.mjs"
```

- [ ] **Step 2: Add a report that walks the static shell and dashboard manifest graph**

Implement `ui/scripts/report-initial-bundle.mjs` with this complete logic:

```js
import { gzipSync } from "node:zlib";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const dist = resolve(import.meta.dirname, "../dist");
const manifest = JSON.parse(
  readFileSync(resolve(dist, ".vite/manifest.json"), "utf8"),
);
const budgetIndex = process.argv.indexOf("--budget");
const budget = budgetIndex === -1 ? undefined : Number(process.argv[budgetIndex + 1]);

const normalizeKey = (key) => key.replaceAll("\\", "/").replace(/^\/+/, "");
const findSourceKey = (source) =>
  Object.keys(manifest).find((key) => {
    const normalized = normalizeKey(key);
    return normalized === source || normalized.endsWith(`/${source}`);
  });
const mainKey = Object.hasOwn(manifest, "index.html")
  ? "index.html"
  : findSourceKey("src/main.tsx");
const dashboardKey = findSourceKey("src/pages/dashboard/index.tsx");
if (!mainKey || !dashboardKey) {
  throw new Error(`Missing manifest entries: main=${mainKey}, dashboard=${dashboardKey}`);
}

const files = new Set();
const visit = (key) => {
  const entry = manifest[key];
  if (!entry || files.has(entry.file)) return;
  files.add(entry.file);
  for (const imported of entry.imports ?? []) visit(imported);
};
visit(mainKey);
visit(dashboardKey);

const markers = {
  monaco: /MonacoEnvironment|typescriptDefaults|editor\.createModel/,
  prettier: /formatWithCursor|prettier\/standalone/,
  recharts: /CartesianGrid|ResponsiveContainer|recharts/,
  xterm: /scrollback|attachCustomWheelEventHandler|xterm/,
};
const assets = [...files]
  .filter((file) => file.endsWith(".js"))
  .map((file) => {
    const body = readFileSync(resolve(dist, file));
    return {
      file,
      bytes: body.byteLength,
      gzipBytes: gzipSync(body).byteLength,
      markers: Object.entries(markers)
        .filter(([, pattern]) => pattern.test(body.toString("utf8")))
        .map(([name]) => name),
    };
  });
const result = {
  mainKey,
  dashboardKey,
  bytes: assets.reduce((sum, asset) => sum + asset.bytes, 0),
  gzipBytes: assets.reduce((sum, asset) => sum + asset.gzipBytes, 0),
  assets,
};
console.log(JSON.stringify(result, null, 2));
if (budget !== undefined && result.gzipBytes >= budget) process.exitCode = 1;
```

- [ ] **Step 3: Add endpoint-family classification and a marker-validated HAR summarizer**

Implement `ui/scripts/network-endpoint-families.mjs` with three literal,
reviewable sets. `profile` contains the eleven resource-list endpoints plus
`ListAllDockerContainers` and `ListTerminals` owned by closed OmniSearch.
`dashboard` is the union of that Omni set with all eleven `Get*Summary`
endpoints plus `GetStackActionState`, `GetDeploymentActionState`, and
`GetStack` owned by the summary and compact-card changes. `none` is empty. Do not infer these sets from
observed traffic: a newly introduced endpoint must be classified in review.

```js
const paths = (names) => new Set(names.map((name) => `/read/${name}`));

const profile = paths([
  "ListServers", "ListSwarms", "ListStacks", "ListDeployments",
  "ListBuilds", "ListRepos", "ListProcedures", "ListActions",
  "ListBuilders", "ListAlerters", "ListResourceSyncs",
  "ListAllDockerContainers", "ListTerminals",
]);
const dashboardSpecific = paths([
  "GetServersSummary", "GetSwarmsSummary", "GetStacksSummary",
  "GetDeploymentsSummary", "GetBuildsSummary", "GetReposSummary",
  "GetProceduresSummary", "GetActionsSummary", "GetBuildersSummary",
  "GetAlertersSummary", "GetResourceSyncsSummary",
  "GetStackActionState", "GetDeploymentActionState", "GetStack",
]);

export const OWNED_ENDPOINT_FAMILIES = {
  none: new Set(),
  profile,
  dashboard: new Set([...profile, ...dashboardSpecific]),
};
```

Implement `ui/scripts/summarize-network-har.mjs` as follows. It accepts the
workload-specific family name and rejects a HAR unless it contains exactly one
ordered start/end pair with the same identifier. Only requests whose start
time lies in the requested interval after the start marker are counted:

```js
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { OWNED_ENDPOINT_FAMILIES } from "./network-endpoint-families.mjs";

export function summarizeHar(harPath, seconds = 60, ownedFamily = "none") {
  if (!Number.isFinite(seconds) || seconds <= 0) {
    throw new Error(`seconds must be positive and finite: ${seconds}`);
  }
  const ownedEndpoints = OWNED_ENDPOINT_FAMILIES[ownedFamily];
  if (!ownedEndpoints) throw new Error(`unknown endpoint family: ${ownedFamily}`);
  const har = JSON.parse(readFileSync(harPath, "utf8"));
  const entries = har?.log?.entries;
  if (!Array.isArray(entries) || entries.length === 0) {
    throw new Error(`${harPath}: HAR has no entries; zero traffic is not proof of a timed window`);
  }
  const started = entries.map((entry) => Date.parse(entry.startedDateTime));
  if (started.some((timestamp) => !Number.isFinite(timestamp))) {
    throw new Error(`${harPath}: HAR contains an invalid startedDateTime`);
  }
  const markers = entries.flatMap((entry, index) => {
    const value = new URL(entry.request.url).searchParams.get("komodo-har-window");
    const match = value?.match(/^(start|end)-(.+)$/);
    return match ? [{ kind: match[1], id: match[2], index, at: started[index] }] : [];
  });
  const starts = markers.filter((marker) => marker.kind === "start");
  const ends = markers.filter((marker) => marker.kind === "end");
  if (starts.length !== 1 || ends.length !== 1) {
    throw new Error(`${harPath}: expected exactly one start and one end marker`);
  }
  const [start] = starts;
  const [end] = ends;
  if (start.id !== end.id) throw new Error(`${harPath}: marker ids do not match`);
  if (start.at >= end.at || start.index >= end.index) {
    throw new Error(`${harPath}: marker pair is reversed`);
  }
  const coverageMs = end.at - start.at;
  if (coverageMs < seconds * 1000) {
    throw new Error(
      `${harPath}: HAR covers ${(coverageMs / 1000).toFixed(3)}s, expected at least ${seconds}s`,
    );
  }
  const cutoff = start.at + seconds * 1000;
  const reads = entries.filter((entry, index) => {
    const pathname = new URL(entry.request.url).pathname;
    return (
      index > start.index &&
      index < end.index &&
      started[index] >= start.at &&
      started[index] < cutoff &&
      (pathname === "/user" || pathname.startsWith("/read/"))
    );
  });
  const byEndpoint = {};
  for (const entry of reads) {
    const pathname = new URL(entry.request.url).pathname;
    byEndpoint[pathname] = (byEndpoint[pathname] ?? 0) + 1;
  }
  const ownedRequestCount = reads.filter((entry) =>
    ownedEndpoints.has(new URL(entry.request.url).pathname),
  ).length;
  return {
    seconds,
    markerId: start.id,
    observedSeconds: coverageMs / 1000,
    requestCount: reads.length,
    ownedRequestCount,
    untouchedRequestCount: reads.length - ownedRequestCount,
    transferBytes: reads.reduce(
      (sum, entry) => sum + Math.max(0, entry.response._transferSize ?? 0),
      0,
    ),
    byEndpoint: Object.fromEntries(
      Object.entries(byEndpoint).sort((left, right) => right[1] - left[1]),
    ),
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [harPath, secondsText = "60", ownedFamily = "none"] = process.argv.slice(2);
  if (!harPath) {
    throw new Error("usage: summarize-network-har.mjs HAR_PATH [SECONDS] [OWNED_FAMILY]");
  }
  console.log(JSON.stringify(
    summarizeHar(harPath, Number(secondsText), ownedFamily),
    null,
    2,
  ));
}
```

Implement `ui/scripts/summarize-network-har.test.mjs` with generated temporary
HAR fixtures. Tests must reject missing, duplicate, reversed, mismatched-id,
and shorter-than-60-second markers; prove pre-start traffic (including an entry
with the exact same millisecond timestamp but a lower HAR index) is excluded; prove
a true zero-read window passes; and prove profile/dashboard owned counts plus
untouched counts are classified exactly. Run `rtk yarn --cwd ui test:har` before
capturing any baseline.

Every capture must contain explicit non-counted start/end requests so a true
zero-read interval is still measurable. After clearing Network, run this in the
browser console and export the HAR only after it resolves:

```js
const komodoHarWindow = crypto.randomUUID();
await fetch(`/favicon.ico?komodo-har-window=start-${komodoHarWindow}`, {
  cache: "no-store",
});
await new Promise((resolve) => setTimeout(resolve, 61_000));
await fetch(`/favicon.ico?komodo-har-window=end-${komodoHarWindow}`, {
  cache: "no-store",
});
```

The marker requests are not `/user` or `/read/*`, so they prove one exact
sixty-second window without changing the counted result.

- [ ] **Step 4: Add an executable three-sample median and threshold gate**

Implement `ui/scripts/check-network-gates.mjs` with this complete logic:

```js
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { summarizeHar } from "./summarize-network-har.mjs";

const [specPath] = process.argv.slice(2);
if (!specPath) throw new Error("usage: check-network-gates.mjs SPEC_JSON");
const specFile = resolve(specPath);
const specDirectory = dirname(specFile);
const spec = JSON.parse(readFileSync(specFile, "utf8"));
const seconds = spec.seconds ?? 60;
const groups = spec.groups ?? {};
if (Object.keys(groups).length === 0) throw new Error("gate spec has no groups");

const median = (values) => [...values].sort((a, b) => a - b)[1];
const samples = {};
const medians = {};
for (const [name, group] of Object.entries(groups)) {
  const { paths, ownedFamily = "none" } = group;
  if (!Array.isArray(paths) || paths.length !== 3) {
    throw new Error(`${name}: exactly three HAR paths are required`);
  }
  samples[name] = paths.map(
    (path) => summarizeHar(resolve(specDirectory, path), seconds, ownedFamily),
  );
  medians[name] = {
    total: median(samples[name].map((sample) => sample.requestCount)),
    owned: median(samples[name].map((sample) => sample.ownedRequestCount)),
    untouched: median(samples[name].map((sample) => sample.untouchedRequestCount)),
  };
}

const requireMedian = (name, metric) => {
  if (!(name in medians)) throw new Error(`unknown HAR group: ${name}`);
  if (!(metric in medians[name])) throw new Error(`unknown metric: ${metric}`);
  return medians[name][metric];
};
const results = [];
for (const gate of spec.gates ?? []) {
  const metric = gate.metric ?? "total";
  const actual = requireMedian(gate.actual, metric);
  const baseline = requireMedian(gate.baseline, metric);
  let limit;
  let passed;
  if (gate.kind === "max-fraction") {
    limit = Math.floor(baseline * gate.fraction);
    passed = actual <= limit;
  } else if (gate.kind === "owned-budget-total") {
    if (metric !== "total") throw new Error("owned-budget-total requires total");
    limit =
      requireMedian(gate.baseline, "untouched") +
      Math.floor(requireMedian(gate.baseline, "owned") * gate.fraction);
    passed = actual <= limit;
  } else if (gate.kind === "max-increase") {
    const allowance = Math.max(gate.requests, baseline * gate.fraction);
    limit = baseline + allowance;
    passed = actual <= limit;
  } else if (gate.kind === "absolute-delta") {
    limit = Math.max(gate.requests, baseline * gate.fraction);
    passed = Math.abs(actual - baseline) <= limit;
  } else {
    throw new Error(`unknown gate kind: ${gate.kind}`);
  }
  results.push({ ...gate, actual, baseline, limit, passed });
}

console.log(JSON.stringify({ seconds, samples, medians, gates: results }, null, 2));
if (results.some((result) => !result.passed)) process.exitCode = 1;
```

Create `docs/superpowers/evidence/2026-07-10-komodo-ui-network-gates.json`
with `seconds: 60`, named groups containing exactly three HAR paths plus one
literal `ownedFamily`, and `gates`. Use `max-increase` on metric `total` for
checkpoint-2 disconnected comparisons; `max-fraction` on metric `owned` with
`fraction: 0.2` for synchronized profile and dashboard owned-family targets;
`owned-budget-total` with `fraction: 0.2` for the separate dashboard total
budget; and `absolute-delta` on `total` for checkpoint-3 disconnected/old-Core
compatibility. Every 5%/one-request gate uses `fraction: 0.05` and
`requests: 1`. Task 1 begins with `gates: []`; the helper still freezes total,
owned, and untouched medians for all four baselines. Store raw HARs
under ignored `target/ui-performance-hars/` and use paths relative to the spec,
for example `../../../target/ui-performance-hars/checkpoint1/profile-connected-1.har`.
Start with these canonical group names and expand the three paths in each
array; later checkpoints append groups without renaming the frozen baselines:

```json
{
  "seconds": 60,
  "groups": {
    "checkpoint1.profile.connected": {
      "ownedFamily": "profile",
      "paths": ["<HAR 1>", "<HAR 2>", "<HAR 3>"]
    },
    "checkpoint1.profile.disconnected": {
      "ownedFamily": "profile",
      "paths": ["<HAR 1>", "<HAR 2>", "<HAR 3>"]
    },
    "checkpoint1.dashboard.connected": {
      "ownedFamily": "dashboard",
      "paths": ["<HAR 1>", "<HAR 2>", "<HAR 3>"]
    },
    "checkpoint1.dashboard.disconnected": {
      "ownedFamily": "dashboard",
      "paths": ["<HAR 1>", "<HAR 2>", "<HAR 3>"]
    }
  },
  "gates": []
}
```

- [ ] **Step 5: Capture the failing production bundle baseline**

Run three times from a clean `ui/dist`, recording the host, commit, Node version, elapsed build time, JSON output, and median in the evidence file:

```bash
rtk yarn --cwd ui test:har
rtk yarn --cwd ui --frozen-lockfile
rtk yarn --cwd ui build
rtk yarn --cwd ui analyze:bundle -- --budget 900000
```

Expected now: the build succeeds, the budget command exits 1, total gzip is near the measured 1.923 MB reference, and the report lists all four heavy markers. If the new baseline differs by more than 10%, explain the changed commit or environment before continuing.

- [ ] **Step 6: Capture three real browser samples for each workload**

Run the production preview:

```bash
rtk yarn --cwd ui preview
```

Expected: Vite serves the built UI at `http://localhost:5173`. In an authenticated browser, disable cache, fix dashboard to Recents view, record resource counts and `showServerStats`, wait for requests to settle, clear Network, run the 61-second marker snippet above, and export HAR after it resolves. For disconnected samples, block only `*/ws/update`, reload, confirm HTTP remains online and the socket indicator is disconnected, then clear Network. Do not use browser Offline mode.

Parse all twelve HARs with commands of this form:

```bash
rtk node ui/scripts/summarize-network-har.mjs target/ui-performance-hars/checkpoint1/dashboard-connected-1.har 60 dashboard
rtk node ui/scripts/summarize-network-har.mjs target/ui-performance-hars/checkpoint1/dashboard-disconnected-1.har 60 dashboard
rtk node ui/scripts/summarize-network-har.mjs target/ui-performance-hars/checkpoint1/profile-connected-1.har 60 profile
rtk node ui/scripts/summarize-network-har.mjs target/ui-performance-hars/checkpoint1/profile-disconnected-1.har 60 profile
rtk yarn --cwd ui analyze:har-gates -- ../docs/superpowers/evidence/2026-07-10-komodo-ui-network-gates.json
```

Expected: all twelve HARs pass the nonempty/coverage checks, the gate helper
prints four three-sample tables and exact medians, and those results are
committed to the evidence file. Label 76/130/338 only as static-cadence
estimates alongside these measured values.

- [ ] **Step 7: Commit measurement only**

```bash
rtk git add ui/vite.config.ts ui/package.json ui/scripts/report-initial-bundle.mjs ui/scripts/network-endpoint-families.mjs ui/scripts/summarize-network-har.mjs ui/scripts/summarize-network-har.test.mjs ui/scripts/check-network-gates.mjs docs/superpowers/evidence/2026-07-10-komodo-ui-startup-background-traffic.md docs/superpowers/evidence/2026-07-10-komodo-ui-network-gates.json
rtk git commit -m "test(ui): capture startup and traffic baselines"
```

Expected: one measurement commit with no behavior change beyond emitting `.vite/manifest.json`.

### Task 2: Split static resource metadata and query hooks from implementations

**Files:**
- Create: `ui/src/resources/types.ts`
- Create: `ui/src/resources/metadata.ts`
- Create: `ui/src/resources/read-hooks.ts`
- Create: `ui/src/resources/implementation.tsx`
- Create: `ui/src/components/lazy-feature.tsx`
- Modify: `ui/src/router.tsx`
- Modify: `ui/src/app/index.tsx`
- Modify: `ui/src/resources/index.ts`
- Modify: `ui/src/lib/hooks.ts`
- Modify: `ui/src/lib/socket.tsx`
- Modify: `ui/src/pages/resources.tsx`
- Modify: `ui/src/pages/resource.tsx`
- Modify: `ui/src/pages/dashboard/tables.tsx`
- Modify: `ui/src/pages/dashboard/recents.tsx`
- Modify: `ui/src/resources/name.tsx`
- Modify: `ui/src/resources/link.tsx`
- Modify: `ui/src/resources/selector.tsx`
- Modify: `ui/src/resources/tags.tsx`
- Modify: `ui/src/resources/not-found.tsx`
- Modify: `ui/src/components/alerts/details.tsx`
- Modify: `ui/src/components/updates/details.tsx`
- Modify: `ui/src/components/permissions/base-section.tsx`
- Modify: `ui/src/components/permissions/specific-section.tsx`
- Modify: `ui/src/resources/build/tabs.tsx`
- Modify: `ui/src/resources/server/resources.tsx`
- Modify: `ui/src/resources/swarm/resources.tsx`
- Modify: every caller returned by `rtk rg -n 'import \{ use(Server|Swarm|Stack|Deployment|Build|Repo|Procedure|Action|Builder|Alerter|ResourceSync|Full)' ui/src`

- [ ] **Step 1: Move types without importing implementation values**

Move `UsableResourceTarget`, `UsableResource`, and `RequiredResourceComponents` from `ui/src/resources/index.ts` to `ui/src/resources/types.ts`. Make `Types`, React, `BoxProps`, `TableProps`, and `PieChartItem` type-only imports. Keep the existing `RequiredResourceComponents` property signatures unchanged so this is cache/API-neutral.

- [ ] **Step 2: Create the complete lightweight metadata registry**

Implement `ui/src/resources/metadata.ts` with no import from a resource directory:

```ts
import { ICONS } from "@/lib/icons";
import type { LucideIcon } from "lucide-react";
import type { Types } from "komodo_client";
import type { UsableResource, UsableResourceTarget } from "./types";

export const RESOURCE_TARGETS = [
  "Server",
  "Swarm",
  "Stack",
  "Deployment",
  "Build",
  "Repo",
  "Procedure",
  "Action",
  "Builder",
  "Alerter",
  "ResourceSync",
] as const satisfies readonly UsableResource[];

export const SETTINGS_RESOURCES: readonly UsableResource[] = ["Builder", "Alerter"];
export const SIDEBAR_RESOURCES = RESOURCE_TARGETS.filter(
  (type) => !SETTINGS_RESOURCES.includes(type),
);

export type ResourceMetadata = {
  label: string;
  plural: string;
  path: string;
  Icon: LucideIcon;
};

export const RESOURCE_METADATA: Record<UsableResource, ResourceMetadata> = {
  Server: { label: "Server", plural: "Servers", path: "servers", Icon: ICONS.Server },
  Swarm: { label: "Swarm", plural: "Swarms", path: "swarms", Icon: ICONS.Swarm },
  Stack: { label: "Stack", plural: "Stacks", path: "stacks", Icon: ICONS.Stack },
  Deployment: { label: "Deployment", plural: "Deployments", path: "deployments", Icon: ICONS.Deployment },
  Build: { label: "Build", plural: "Builds", path: "builds", Icon: ICONS.Build },
  Repo: { label: "Repo", plural: "Repos", path: "repos", Icon: ICONS.Repo },
  Procedure: { label: "Procedure", plural: "Procedures", path: "procedures", Icon: ICONS.Procedure },
  Action: { label: "Action", plural: "Actions", path: "actions", Icon: ICONS.Action },
  Builder: { label: "Builder", plural: "Builders", path: "builders", Icon: ICONS.Builder },
  Alerter: { label: "Alerter", plural: "Alerters", path: "alerters", Icon: ICONS.Alerter },
  ResourceSync: { label: "Resource Sync", plural: "Resource Syncs", path: "resource-syncs", Icon: ICONS.ResourceSync },
};

const RESOURCE_TYPES = new Set<string>(RESOURCE_TARGETS);
export function isUsableResourceTarget(
  target: Types.ResourceTarget,
): target is UsableResourceTarget {
  return RESOURCE_TYPES.has(target.type);
}
```

- [ ] **Step 3: Extract typed list/full read hooks so dynamic resources do not import one another's indexes**

In `ui/src/resources/read-hooks.ts`, move the existing `useServer`, `useFullServer`, `useSwarm`, `useFullSwarm`, `useStack`, `useFullStack`, `useDeployment`, `useFullDeployment`, `useBuild`, `useFullBuild`, `useRepo`, `useFullRepo`, `useProcedure`, `useFullProcedure`, `useAction`, `useFullAction`, `useBuilder`, `useFullBuilder`, `useAlerter`, `useFullAlerter`, `useResourceSync`, and `useFullResourceSync` bodies unchanged. Add these generic stable-key helpers:

```ts
const RESOURCE_LIST_QUERIES = {
  Server: "ListServers",
  Swarm: "ListSwarms",
  Stack: "ListStacks",
  Deployment: "ListDeployments",
  Build: "ListBuilds",
  Repo: "ListRepos",
  Procedure: "ListProcedures",
  Action: "ListActions",
  Builder: "ListBuilders",
  Alerter: "ListAlerters",
  ResourceSync: "ListResourceSyncs",
} as const satisfies Record<UsableResource, Types.ReadRequest["type"]>;

export type ResourceListItemByType = {
  Server: Types.ServerListItem;
  Swarm: Types.SwarmListItem;
  Stack: Types.StackListItem;
  Deployment: Types.DeploymentListItem;
  Build: Types.BuildListItem;
  Repo: Types.RepoListItem;
  Procedure: Types.ProcedureListItem;
  Action: Types.ActionListItem;
  Builder: Types.BuilderListItem;
  Alerter: Types.AlerterListItem;
  ResourceSync: Types.ResourceSyncListItem;
};

export function useResourceList<T extends UsableResource>(
  type: T,
  enabled = true,
): ResourceListItemByType[T][] | undefined {
  return useRead(RESOURCE_LIST_QUERIES[type], {}, { enabled }).data as
    | ResourceListItemByType[T][]
    | undefined;
}

export function useResourceListItem<T extends UsableResource>(
  type: T,
  id: string | undefined,
  useName = false,
  enabled = true,
): ResourceListItemByType[T] | undefined {
  return useResourceList(type, enabled)?.find((resource) =>
    useName ? resource.name === id : resource.id === id,
  );
}
```

The one cast is at the generated-client dispatch boundary, while all consumers
receive a concrete list-item type.

Replace cross-resource imports from `@/resources/server`, `@/resources/stack`, and the other resource indexes with `@/resources/read-hooks`. Remove the moved function bodies from each resource index and import them back only to populate its implementation object. Query keys remain `[type, params]`.

- [ ] **Step 4: Add one loading/error boundary for lazy features**

Implement `ui/src/components/lazy-feature.tsx` as this class error boundary wrapping `Suspense`:

```tsx
import { Alert, Button, Stack } from "@mantine/core";
import { LoadingScreen } from "mogh_ui";
import {
  Component,
  Suspense,
  type ErrorInfo,
  type ReactNode,
} from "react";

export type LazyFeatureProps = {
  name: string;
  children: ReactNode;
  fallback?: ReactNode;
};

type BoundaryProps = Pick<LazyFeatureProps, "name" | "children">;
type BoundaryState = { error: Error | undefined };

class LazyFeatureBoundary extends Component<BoundaryProps, BoundaryState> {
  state: BoundaryState = { error: undefined };

  static getDerivedStateFromError(error: Error): BoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Lazy feature failed", error, info);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <Alert color="red" title={`Unable to load ${this.props.name}`}>
        <Stack align="flex-start">
          <div>{this.state.error.message}</div>
          <Button onClick={() => window.location.reload()}>Retry</Button>
        </Stack>
      </Alert>
    );
  }
}

export default function LazyFeature({
  name,
  children,
  fallback = <LoadingScreen />,
}: LazyFeatureProps): ReactNode {
  return (
    <LazyFeatureBoundary name={name}>
      <Suspense fallback={fallback}>{children}</Suspense>
    </LazyFeatureBoundary>
  );
}
```

The explicit reload is intentional: React caches a rejected `lazy()` import, so merely clearing boundary state does not reliably retry the chunk. Do not silently return `null` on chunk failure.

Make this boundary own every existing route-level `lazy()` as well. In
`router.tsx`, wrap all current authentication branches and `<BrowserRouter>` in
one outer boundary; the direct passkey/TOTP `<Login />` path must be inside it.
Rename the current `export const Router = () => {` declaration to
`const RouterContent = () => {` without changing its body. After that
component's closing `};`, add:

```tsx
import LazyFeature from "@/components/lazy-feature";

export const Router = () => (
  <LazyFeature name="route">
    <RouterContent />
  </LazyFeature>
);
```

Remove the now-redundant `Suspense` import/wrapper from `app/index.tsx`; its
`Outlet`, `UpdateDetails`, and `AlertDetails` remain inside `AppShell.Main` and
are covered by the router-owned loading/error boundary. This is required for
both unauthenticated lazy Login and authenticated route chunks; do not leave a
lazy route with only a loading boundary or no boundary.

- [ ] **Step 5: Add the complete dynamic implementation map**

Implement `ui/src/resources/implementation.tsx` with this shape and all eleven explicit imports:

```tsx
import LazyFeature from "@/components/lazy-feature";
import { lazy, type ReactNode } from "react";
import type { RequiredResourceComponents, UsableResource } from "./types";

type RenderProps = {
  children: (components: RequiredResourceComponents) => ReactNode;
};
type Loader = () => Promise<RequiredResourceComponents>;

const asLazy = (loader: Loader) =>
  lazy(async () => {
    const components = await loader();
    return { default: ({ children }: RenderProps) => children(components) };
  });

const IMPLEMENTATIONS: Record<UsableResource, ReturnType<typeof asLazy>> = {
  Server: asLazy(() => import("./server").then((module) => module.ServerComponents)),
  Swarm: asLazy(() => import("./swarm").then((module) => module.SwarmComponents)),
  Stack: asLazy(() => import("./stack").then((module) => module.StackComponents)),
  Deployment: asLazy(() => import("./deployment").then((module) => module.DeploymentComponents)),
  Build: asLazy(() => import("./build").then((module) => module.BuildComponents)),
  Repo: asLazy(() => import("./repo").then((module) => module.RepoComponents)),
  Procedure: asLazy(() => import("./procedure").then((module) => module.ProcedureComponents)),
  Action: asLazy(() => import("./action").then((module) => module.ActionComponents)),
  Builder: asLazy(() => import("./builder").then((module) => module.BuilderComponents)),
  Alerter: asLazy(() => import("./alerter").then((module) => module.AlerterComponents)),
  ResourceSync: asLazy(() => import("./sync").then((module) => module.ResourceSyncComponents)),
};

export function ResourceImplementation({
  type,
  children,
}: RenderProps & { type: UsableResource }) {
  const Implementation = IMPLEMENTATIONS[type];
  return (
    <LazyFeature key={type} name={`${type} resource`}>
      <Implementation>{children}</Implementation>
    </LazyFeature>
  );
}
```

Make `ui/src/resources/index.ts` a lightweight barrel that exports only `types`, `metadata`, `read-hooks`, and `implementation`; it must not import any `./server`, `./stack`, or other implementation directory.
In every resource `index.tsx`, replace the current parent/barrel import with
`import type { RequiredResourceComponents } from "../types"`; the Sync, Stack,
and Repo directories must not retain `from "@/resources"`, which would create a
cycle back through the dynamic implementation registry.

- [ ] **Step 6: Migrate static consumers without calling hooks inside render props**

In `pages/resources.tsx`, `pages/resource.tsx`, and `pages/dashboard/tables.tsx`, wrap a small loaded child component with `ResourceImplementation`; the child receives `RC` as a prop and owns every `RC.use*` hook call. Never invoke an implementation hook directly inside the `children={(RC) => ...}` render callback.

Use `RESOURCE_METADATA[type].Icon` and `useResourceListItem` in socket notifications, names, links, selectors, tags, not-found UI, alerts/updates, and permission tables. For `System`, use `isUsableResourceTarget` rather than looking up an implementation. In `build/tabs.tsx`, `server/resources.tsx`, and `swarm/resources.tsx`, replace global registry access with the already-existing `NewResource`, `NewResourceWithDeployTarget`, and direct table components while preserving the currently honored props; do not opportunistically change the ignored Repo `serverId` or Deployment `buildId` behavior.

`RESOURCE_METADATA[type].Icon` is a bare `LucideIcon`, not the old hook-backed
resource icon. In `ResourceLink`, keep `noColor` in the public props for caller
compatibility but render exactly `<Icon size={iconSize} />`; do not forward
`id` or `noColor`, because neither is a Lucide prop. Static links intentionally
use the neutral current-color icon so they do not pull state hooks or a resource
implementation chunk. Resource tables and headers retain their existing
state-colored icons and badges.

- [ ] **Step 7: Prove the static registry is gone before committing**

```bash
rtk rg -n -w "ResourceComponents" ui/src
rtk rg -n 'from "@/resources/(server|swarm|stack|deployment|build|repo|procedure|action|builder|alerter|sync)"' ui/src/lib ui/src/app
rtk rg -n 'LazyFeature name="route"' ui/src/router.tsx
rtk yarn --cwd ui build
```

Expected: the first two searches return no static global implementation
consumer, the route boundary search finds exactly one owner, and the build
exits 0. Dynamic imports in `resources/implementation.tsx` are expected.

- [ ] **Step 8: Commit the metadata/implementation split**

```bash
rtk git add ui/src/resources ui/src/lib/hooks.ts ui/src/lib/socket.tsx ui/src/pages ui/src/components ui/src/router.tsx ui/src/app/index.tsx
rtk git commit -m "refactor(ui): split resource metadata from implementations"
```

Expected: the commit contains no query-key or polling-interval change.

### Task 3: Lazy-load Monaco, typings, Prettier, Recharts, and xterm at activation boundaries

**Files:**
- Create: `ui/patches/mogh_ui+0.6.1.patch`
- Create: `ui/src/components/lazy-monaco.tsx`
- Modify: `ui/package.json`
- Modify: `ui/yarn.lock`
- Modify: `ui/Dockerfile`
- Modify: `ui/src/main.tsx`
- Modify: `ui/src/monaco.ts`
- Modify: `ui/src/resources/server/stats/index.tsx`
- Modify: `ui/src/components/terminal/section.tsx`
- Modify: every file returned by `rtk rg -l 'Monaco(Editor|DiffEditor)|languageFromPath' ui/src`

- [ ] **Step 1: Prepare the package split in the same uncommitted unit as editor migration**

The locked `mogh_ui@0.6.1` root entry exports `./components/monaco`, which
imports Monaco/Prettier and runs `./init`. Keep version `0.6.1`, add exact dev
dependency `patch-package@8.0.0`, and add `"postinstall": "patch-package"`.
Generate `ui/patches/mogh_ui+0.6.1.patch` that removes only the Monaco export
from `dist/index.js`, `index.cjs`, `index.d.ts`, and `index.d.cts`; adds package
export `./monaco` pointing to `dist/components/monaco/index.{js,cjs,d.ts,d.cts}`
for matching import/require/types conditions; and preserves all existing root
and SCSS exports.

In `ui/Dockerfile`, make the dependency layer see the patch before postinstall:

```dockerfile
COPY ./ui/package.json ./ui/yarn.lock ./ui/
COPY ./ui/patches ./ui/patches
RUN cd ui && yarn --frozen-lockfile
```

Run the patch-generation commands, but do not build or commit until every old
editor import has been migrated in Steps 2–4; removing root exports before that
migration is intentionally an intermediate RED state inside one atomic commit.

```bash
rtk yarn --cwd ui add --dev --exact patch-package@8.0.0
# Edit only ui/node_modules/mogh_ui/package.json and the four root index files.
rtk yarn --cwd ui patch-package mogh_ui
rtk rm -rf ui/node_modules
rtk yarn --cwd ui --frozen-lockfile
rtk rg -n 'components/monaco' ui/node_modules/mogh_ui/dist/index.{js,cjs,d.ts,d.cts}
rtk yarn --cwd ui node -e 'import("mogh_ui/monaco").then(m => { if (!m.MonacoEditor || !m.MonacoDiffEditor) process.exit(1) })'
```

Expected: the root search returns no match, the subpath import succeeds from
the UI package, and a frozen local install reapplies the patch. Root imports
such as `LoadingScreen` remain supported without Monaco side effects.

- [ ] **Step 2: Turn Monaco initialization into a cached activation promise**

Remove the import and call from `ui/src/main.tsx`. In `ui/src/monaco.ts`, retain the existing YAML configuration and compiler/diagnostic options, but export this cache around the existing initializer and make each fetch reject on a non-2xx response:

```ts
let monacoInitialization: Promise<void> | undefined;

export function ensureMonaco(): Promise<void> {
  monacoInitialization ??= initMonaco().catch((error) => {
    monacoInitialization = undefined;
    throw error;
  });
  return monacoInitialization;
}

async function fetchText(path: string): Promise<string> {
  const response = await fetch(path, { cache: "force-cache" });
  if (!response.ok) throw new Error(`Failed to load ${path}: ${response.status}`);
  return response.text();
}
```

Replace each current `fetch(path).then((res) => res.text())` with `fetchText(path)`. The first call still loads the exact ten existing assets; subsequent calls reuse the same promise and browser cache.

- [ ] **Step 3: Add lazy editor wrappers and a local path-language helper**

Implement `ui/src/components/lazy-monaco.tsx` completely as follows. The
`ComponentProps` aliases point at the patched explicit Monaco subpath and are
type-only; both runtime imports stay inside `lazy` factories. Never dynamically
import the `mogh_ui` root for an editor.

```tsx
import LazyFeature from "@/components/lazy-feature";
import { lazy, type ComponentProps, type ReactNode } from "react";

export type MonacoEditorProps = ComponentProps<
  typeof import("mogh_ui/monaco").MonacoEditor
>;
export type MonacoDiffEditorProps = ComponentProps<
  typeof import("mogh_ui/monaco").MonacoDiffEditor
>;
export type MonacoLanguage = MonacoEditorProps["language"];

const MonacoEditor = lazy(async () => {
  const [ui, monaco] = await Promise.all([
    import("mogh_ui/monaco"),
    import("@/monaco"),
  ]);
  await monaco.ensureMonaco();
  return { default: ui.MonacoEditor };
});

const MonacoDiffEditor = lazy(async () => {
  const [ui, monaco] = await Promise.all([
    import("mogh_ui/monaco"),
    import("@/monaco"),
  ]);
  await monaco.ensureMonaco();
  return { default: ui.MonacoDiffEditor };
});

export function LazyMonacoEditor(props: MonacoEditorProps): ReactNode {
  return (
    <LazyFeature name="Monaco editor">
      <MonacoEditor {...props} />
    </LazyFeature>
  );
}

export function LazyMonacoDiffEditor(
  props: MonacoDiffEditorProps,
): ReactNode {
  return (
    <LazyFeature name="Monaco diff editor">
      <MonacoDiffEditor {...props} />
    </LazyFeature>
  );
}

const EXTENSIONS: ReadonlyArray<[readonly string[], MonacoLanguage]> = [
  [[".yaml", ".yml"], "yaml"],
  [[".toml"], "toml"],
  [[".json"], "json"],
  [[".env", ".conf"], "key_value"],
  [[".ini"], "ini"],
  [[".sh", ".bash", ".zsh"], "shell"],
  [["dockerfile"], "dockerfile"],
  [[".rs"], "rust"],
  [[".js", ".jsx", ".mjs", ".cjs"], "javascript"],
  [[".ts", ".tsx"], "typescript"],
];

export function languageFromPath(
  path: string,
): MonacoLanguage | undefined {
  const normalized = path.toLowerCase();
  return EXTENSIONS.find(([extensions]) =>
    extensions.some((extension) => normalized.endsWith(extension)),
  )?.[1];
}
```

This local helper prevents a static barrel import solely for
`languageFromPath`.

- [ ] **Step 4: Replace every editor value import**

Run the inventory first:

```bash
rtk rg -l 'Monaco(Editor|DiffEditor)|languageFromPath' ui/src
```

Replace value imports in `components/export-toml.tsx`, `components/inspect-section.tsx`, `components/alerts/details.tsx`, `components/updates/details.tsx`, `components/config/system-command.tsx`, all Build/Repo/Action/Alerter/Procedure/Deployment/Stack/Sync editor files, settings Core info, and Docker/Swarm inspect/config/secret pages with `LazyMonacoEditor`, `LazyMonacoDiffEditor`, and the local `languageFromPath`. Do not change editor props or values.

- [ ] **Step 5: Put Recharts and xterm behind their actual tab activation**

In `ui/src/resources/server/stats/index.tsx`, replace the static historical
import with these declarations and rendered section:

```tsx
import LazyFeature from "@/components/lazy-feature";
import { lazy, type ReactNode } from "react";

const ServerHistoricalStats = lazy(() => import("./historical"));

<LazyFeature name="historical server charts">
  <ServerHistoricalStats id={id} />
</LazyFeature>
```

In `ui/src/components/terminal/section.tsx`, replace the static target import
and the all-terminals render loop with:

```tsx
import LazyFeature from "@/components/lazy-feature";
import { lazy, useState } from "react";

const TargetTerminal = lazy(() => import("./target"));

const selectedTerminal = terminals?.find(
  (terminal) => terminal.name === selected,
);

{selectedTerminal && (
  <LazyFeature key={selectedTerminal.name} name="terminal">
    <TargetTerminal
      terminal={selectedTerminal.name}
      target={
        target.type === "Server" ? target : selectedTerminal.target
      }
      selected
      _reconnect={_reconnect}
    />
  </LazyFeature>
)}
```

This makes opening one saved terminal the xterm activation boundary instead of
mounting every saved terminal. The already route-lazy
`ui/src/pages/terminal.tsx` may keep its direct target import because opening
that route is terminal activation.

- [ ] **Step 6: Build locally and in the release Docker path, then prove demand loading**

```bash
rtk yarn --cwd ui --frozen-lockfile
rtk rg -n 'components/monaco' ui/node_modules/mogh_ui/dist/index.{js,cjs,d.ts,d.cts}
rtk rg -n "from ['\"]mogh_ui['\"]" ui/src/components/lazy-monaco.tsx
rtk yarn --cwd ui build
rtk docker build --target builder -f ui/Dockerfile .
rtk yarn --cwd ui analyze:bundle -- --budget 900000
rtk rg -n "MonacoEnvironment|formatWithCursor|CartesianGrid|attachCustomWheelEventHandler" ui/dist/assets
```

Expected: local and Docker builder builds exit 0, proving postinstall sees the
patch in the release dependency layer; bundle report exits 0 below 900,000
gzip bytes with no heavy marker in the shell/dashboard static graph; the final
search finds markers only in noninitial lazy chunks.

With cache disabled, load `/` and confirm Network has no Monaco worker/editor, Recharts, xterm, Prettier, or typing requests. Open one editor and confirm its loading boundary, editor functionality, and exactly ten successful typing requests. Reopen with cache enabled and confirm zero additional typing transfer. Open Server Stats, then a terminal, and confirm their chunks load only at those activations and both features function.

- [ ] **Step 7: Commit the atomic package and heavy-feature split**

```bash
rtk git add ui/package.json ui/yarn.lock ui/Dockerfile ui/patches/mogh_ui+0.6.1.patch ui/src/main.tsx ui/src/monaco.ts ui/src/components ui/src/resources ui/src/pages
rtk git commit -m "perf(ui): lazy load heavy UI features"
```

### Task 4: Verify and ship checkpoint 1 with an independent rollback boundary

**Files:**
- Modify: `docs/superpowers/evidence/2026-07-10-komodo-ui-startup-background-traffic.md`

- [ ] **Step 1: Record three post-split builds and the browser feature matrix**

Repeat Task 1's three builds, record the median and per-asset results, and attach browser Network screenshots for initial dashboard, first editor, cached editor reopen, chart activation, and terminal activation. In DevTools, separately block one authenticated route chunk and the Login route chunk: each must show the route loading state, then the named route error boundary with Retry; unblock and press Retry to recover. Also block one feature chunk and prove its local boundary. Do not commit a broken URL.

- [ ] **Step 2: Commit checkpoint-1 evidence**

```bash
rtk git add docs/superpowers/evidence/2026-07-10-komodo-ui-startup-background-traffic.md
rtk git commit -m "docs(ui): record startup chunk evidence"
```

- [ ] **Step 3: Run checkpoint verification**

```bash
rtk yarn --cwd ui --frozen-lockfile
rtk proxy sh -c 'if rtk rg -n "components/monaco" ui/node_modules/mogh_ui/dist/index.{js,cjs,d.ts,d.cts}; then exit 1; fi'
rtk proxy sh -c "if rtk rg -nF -e 'import(\"mogh_ui\")' -e \"import('mogh_ui')\" ui/src/components/lazy-monaco.tsx; then exit 1; fi"
rtk yarn --cwd ui build
rtk yarn --cwd ui analyze:bundle -- --budget 900000
rtk git diff --check main...HEAD
rtk git status --short
```

Expected: both negative source guards and all other commands exit 0, status is
clean, the frozen install proves `patch-package` is
reproducible, initial static dashboard JavaScript contains none of the four
heavy markers and is below budget, and the evidence distinguishes local gzip
from browser transfer sizes.

- [ ] **Step 4: Open the fork-only PR**

```bash
rtk git push -u origin ui-startup-chunks
rtk gh pr create --repo intezya/komodo --base main --head ui-startup-chunks --title "Lazy load UI resource and heavy feature code" --body-file docs/superpowers/evidence/2026-07-10-komodo-ui-startup-background-traffic.md
```

Expected: PR base/head are `intezya/komodo:main` and `intezya/komodo:ui-startup-chunks`. Rollback is a revert of the two behavior commits; there is no storage or cache-key migration.

### Task 5: Mount one Spotlight root and disable every Omni query while closed

**Files:**
- Modify: `ui/src/app/topbar/index.tsx`
- Modify: `ui/src/app/topbar/omni-search/index.tsx`
- Modify: `ui/src/app/topbar/omni-search/hooks.tsx`
- Modify: `ui/src/lib/hooks.ts`

- [ ] **Step 1: Capture the closed/open Omni request delta before editing**

On `/profile`, capture 60 seconds with Spotlight closed, then 60 seconds with it held open. Record `/read/ListAllDockerContainers`, `/read/ListTerminals`, and all eleven resource-list counts in the evidence file. Expected now: the closed state still performs the thirteen 15-second refetches.

- [ ] **Step 2: Split two responsive triggers from one root**

Make `OmniSearchTrigger` a hook-free exported component accepting `hiddenFrom` or `visibleFrom`. Mount its desktop trigger in the center column and mobile trigger in the right group, but mount `<OmniSearch />` exactly once as a sibling after `AppShell.Header` in the `Topbar` fragment. Keep the keyboard listener only in the single root.

- [ ] **Step 3: Gate every Omni observer from Spotlight callbacks**

Use this state flow in `OmniSearch`:

```tsx
const [opened, setOpened] = useState(false);
const { search, setSearch, actions } = useOmniSearch(opened);

<Spotlight.Root
  query={search}
  onQueryChange={setSearch}
  onSpotlightOpen={() => setOpened(true)}
  onSpotlightClose={() => setOpened(false)}
  clearQueryOnClose={false}
  radius="sm"
>
```

Change the hook signature to `useOmniSearch(enabled: boolean)`. Pass `{ enabled, refetchInterval: 15_000 }` to containers and terminals. Change `useAllResources` with this backward-compatible defaulted signature and pass both values to all eleven stable `useRead` calls:

```ts
type AllResourcesOptions = {
  enabled?: boolean;
  refetchInterval?: number | false;
};

export function useAllResources(
  {
    enabled = true,
    refetchInterval,
  }: AllResourcesOptions = {},
): ResourceMap {
```

Change Omni's numeric call to
`useAllResources({ enabled, refetchInterval: 15_000 })`; existing no-argument
callers remain valid. Disabled observers must retain their query keys and
cached data.

- [ ] **Step 4: Verify no closed search traffic and unchanged open behavior**

```bash
rtk yarn --cwd ui build
```

Expected: build exits 0. Browser Network shows zero Omni-owned HTTP requests during a closed 60-second sample, immediate cached results plus any stale refetch when opened, navigation closes Spotlight, and reopening search works at both mobile and desktop breakpoints. React Profiler shows one Spotlight root.

- [ ] **Step 5: Commit the isolated search change**

```bash
rtk git add ui/src/app/topbar ui/src/lib/hooks.ts
rtk git commit -m "perf(ui): suspend closed omni search queries"
```

### Task 6: Reuse dashboard list items and remove duplicate or hidden card work

**Files:**
- Create: `ui/src/resources/dashboard-summary.tsx`
- Create: `ui/src/pages/dashboard/compact-update-badge.tsx`
- Create: `ui/src/resources/action/state.tsx`
- Create: `ui/src/resources/build/state.tsx`
- Create: `ui/src/resources/deployment/state.tsx`
- Create: `ui/src/resources/procedure/state.tsx`
- Create: `ui/src/resources/repo/state.tsx`
- Create: `ui/src/resources/server/state.tsx`
- Create: `ui/src/resources/stack/state.tsx`
- Create: `ui/src/resources/swarm/state.tsx`
- Create: `ui/src/resources/sync/state.tsx`
- Modify: `ui/src/pages/dashboard/recents.tsx`
- Modify: `ui/src/pages/dashboard/tables.tsx`
- Modify: `ui/src/resources/link.tsx`
- Modify: `ui/src/resources/name.tsx`
- Modify: `ui/src/resources/action/table.tsx`
- Modify: `ui/src/resources/build/table.tsx`
- Modify: `ui/src/resources/deployment/table.tsx`
- Modify: `ui/src/resources/procedure/table.tsx`
- Modify: `ui/src/resources/repo/table.tsx`
- Modify: `ui/src/resources/server/table/index.tsx`
- Modify: `ui/src/resources/server/table/standard.tsx`
- Modify: `ui/src/resources/server/table/stats.tsx`
- Modify: `ui/src/resources/stack/table.tsx`
- Modify: `ui/src/resources/swarm/table.tsx`
- Modify: `ui/src/resources/sync/table.tsx`
- Modify: all eleven `ui/src/resources/*/index.tsx` files that currently define `useDashboardSummaryData`

- [ ] **Step 1: Extract lightweight dashboard summaries from full resource implementations**

Move each existing `GetServersSummary`, `GetSwarmsSummary`, `GetStacksSummary`, `GetDeploymentsSummary`, `GetBuildsSummary`, `GetReposSummary`, `GetProceduresSummary`, `GetActionsSummary`, `GetBuildersSummary`, `GetAlertersSummary`, and `GetResourceSyncsSummary` hook and its exact `PieChartItem` transformation into `ui/src/resources/dashboard-summary.tsx`. Export a complete map with this signature:

```ts
export type ResourceDashboardSummaryProps = {
  name: string;
  onClick: () => void;
};

export const RESOURCE_DASHBOARD_SUMMARIES: Record<
  UsableResource,
  ComponentType<ResourceDashboardSummaryProps>
>;
```

Each mapped component uses `RESOURCE_METADATA[type].Icon` and the same summary endpoint. Remove `useDashboardSummaryData` and the unused `DashboardSummary` member from `RequiredResourceComponents` so dashboard recents never resolve full resource implementations.

- [ ] **Step 2: Pass list items through cards instead of querying them again**

In Recents, derive items directly from the already-fetched list response. Do
not rely on narrowing an indexed generic from a separate `type` variable;
TypeScript does not preserve that correlation. Use this real discriminated
union and keep the whole `props` object through each switch:

```tsx
type RecentRowProps = {
  [K in UsableResource]: {
    type: K;
    items: ResourceListItemByType[K][];
  };
}[UsableResource];

type RecentCardProps = {
  [K in UsableResource]: {
    type: K;
    resource: ResourceListItemByType[K];
  };
}[UsableResource];

function RecentCard(props: RecentCardProps) {
  let updateBadge: ReactNode;
  switch (props.type) {
    case "Stack":
      updateBadge = <CompactUpdateBadge type="Stack" resource={props.resource} />;
      break;
    case "Deployment":
      updateBadge = <CompactUpdateBadge type="Deployment" resource={props.resource} />;
      break;
    default:
      updateBadge = undefined;
  }
  return <CommonRecentCard {...props} updateBadge={updateBadge} />;
}
```

Do not call a hook from `SIDEBAR_RESOURCES.map`. Close each literal type inside
a named component, then map only components:

```tsx
const ServerRecentRow = () => <RecentRowView type="Server" items={useResourceList("Server") ?? []} />;
const SwarmRecentRow = () => <RecentRowView type="Swarm" items={useResourceList("Swarm") ?? []} />;
const StackRecentRow = () => <RecentRowView type="Stack" items={useResourceList("Stack") ?? []} />;
const DeploymentRecentRow = () => <RecentRowView type="Deployment" items={useResourceList("Deployment") ?? []} />;
const BuildRecentRow = () => <RecentRowView type="Build" items={useResourceList("Build") ?? []} />;
const RepoRecentRow = () => <RecentRowView type="Repo" items={useResourceList("Repo") ?? []} />;
const ProcedureRecentRow = () => <RecentRowView type="Procedure" items={useResourceList("Procedure") ?? []} />;
const ActionRecentRow = () => <RecentRowView type="Action" items={useResourceList("Action") ?? []} />;
const BuilderRecentRow = () => <RecentRowView type="Builder" items={useResourceList("Builder") ?? []} />;
const AlerterRecentRow = () => <RecentRowView type="Alerter" items={useResourceList("Alerter") ?? []} />;
const ResourceSyncRecentRow = () => <RecentRowView type="ResourceSync" items={useResourceList("ResourceSync") ?? []} />;

const RECENT_ROWS: Record<UsableResource, ComponentType> = {
  Server: ServerRecentRow, Swarm: SwarmRecentRow, Stack: StackRecentRow,
  Deployment: DeploymentRecentRow, Build: BuildRecentRow, Repo: RepoRecentRow,
  Procedure: ProcedureRecentRow, Action: ActionRecentRow,
  Builder: BuilderRecentRow, Alerter: AlerterRecentRow,
  ResourceSync: ResourceSyncRecentRow,
};
```

`RecentRowView(props: RecentRowProps)` switches on `props.type` before mapping
typed cards (or delegates to eleven tiny typed views); it never destructures
`type` away from `items`. Add hook-free `ResourceLinkView` and
`ResourceNameView` overloads accepting concrete list items. No
`ResourceListItem<unknown>` cast is permitted. `rtk yarn --cwd ui build` is the
compile regression for all eleven branches.

- [ ] **Step 3: Render compact update state from the list item only**

Implement `compact-update-badge.tsx` with this hook-free code:

```tsx
import { ICONS } from "@/lib/icons";
import { ActionIcon, Box, HoverCard, Stack, Text } from "@mantine/core";
import { hexColorByIntention } from "mogh_ui";
import type { Types } from "komodo_client";

type Props =
  | { type: "Stack"; resource: Types.StackListItem }
  | { type: "Deployment"; resource: Types.DeploymentListItem };

export default function CompactUpdateBadge(props: Props) {
  const services =
    props.type === "Stack"
      ? props.resource.info.services.filter(
          (service) => service.update_available,
        )
      : [];
  const updateAvailable =
    props.type === "Stack"
      ? services.length > 0
      : props.resource.info.update_available;
  if (!updateAvailable) return null;

  return (
    <Box>
      <HoverCard>
        <HoverCard.Target>
          <ActionIcon
            variant="outline"
            bd={`1px solid ${hexColorByIntention("Neutral")}`}
            size="md"
            aria-label="Update available"
          >
            <ICONS.UpdateAvailable size="1rem" />
          </ActionIcon>
        </HoverCard.Target>
        <HoverCard.Dropdown>
          {props.type === "Deployment" ? (
            "There is a newer image available."
          ) : (
            <Stack gap={0}>
              {services.map((service) => (
                <Text key={service.service}>{service.service}</Text>
              ))}
            </Stack>
          )}
        </HoverCard.Dropdown>
      </HoverCard>
    </Box>
  );
}
```

At the call site, narrow `RecentCard` by `type` before passing the corresponding
`Types.StackListItem` or `Types.DeploymentListItem`. Do not cast an arbitrary
`ResourceListItem<unknown>` into this component. It must not call permissions,
action-state, write, execute, or full-detail hooks.

- [ ] **Step 4: Replace two responsive card trees with one responsive container**

Replace `hiddenFrom="md"` and `visibleFrom="md"` siblings with one:

```tsx
<Flex
  direction={{ base: "column", md: "row" }}
  className="bordered-light"
  bdrs="md"
>
  {children}
</Flex>
```

Mount `ServerStatsCard` only when `preferences.showServerStats` is true; do not hide a mounted querying component with height/opacity. Keep the same card limits and breakpoints.

- [ ] **Step 5: Keep dashboard tables implementation-local**

Change each dashboard table section to lazy-load the direct table module rather
than a resource `index.tsx`. Add this explicit map in
`ui/src/pages/dashboard/tables.tsx`:

```tsx
import LazyFeature from "@/components/lazy-feature";
import type { RequiredResourceComponents, UsableResource } from "@/resources/types";
import { lazy, type LazyExoticComponent } from "react";

type ResourceTable = RequiredResourceComponents["Table"];

const RESOURCE_TABLES: Record<
  UsableResource,
  LazyExoticComponent<ResourceTable>
> = {
  Server: lazy(() => import("@/resources/server/table")),
  Swarm: lazy(() => import("@/resources/swarm/table")),
  Stack: lazy(() => import("@/resources/stack/table")),
  Deployment: lazy(() => import("@/resources/deployment/table")),
  Build: lazy(() => import("@/resources/build/table")),
  Repo: lazy(() => import("@/resources/repo/table")),
  Procedure: lazy(() => import("@/resources/procedure/table")),
  Action: lazy(() => import("@/resources/action/table")),
  Builder: lazy(() => import("@/resources/builder/table")),
  Alerter: lazy(() => import("@/resources/alerter/table")),
  ResourceSync: lazy(() => import("@/resources/sync/table")),
};
```

Render `RESOURCE_TABLES[type]` inside `LazyFeature`. Direct table modules keep
Config, Page, Executions, and editor code out of the dashboard table chunk.

Before using that map, remove every table import of a full
`*Components` object. Extract the nine current status renderers into the
listed lightweight `state.tsx` modules. Each exported badge accepts the
already-owned row `info` (and Core version only for Server) and performs no
read hook. A resource index may keep its public `State({ id })` wrapper by
reading the list item and rendering the pure badge; each table renders the
same badge from `row.original.info`. Builder and Alerter keep their existing
null state and need no module. Server's standard table uses the pure state
badge; its explicitly selected stats table may retain its existing stats
observers, but neither table imports `ServerComponents` or the server index.

Add this source-level chunk guard to checkpoint verification:

```bash
rtk rg -n '[A-Za-z]+Components\.(State|Icon)' ui/src/resources/{action,build,deployment,procedure,repo,server,stack,swarm,sync}/table*
rtk rg -n 'import .*Components.*from "\.\.?"' ui/src/resources/{action,build,deployment,procedure,repo,server,stack,swarm,sync}/table*
```

Expected: both searches return no matches. The Vite manifest must show each
dashboard table chunk imports only its table, lightweight state, metadata,
read-hook, and shared presentation dependencies; it must not statically reach
the corresponding resource `index.tsx`.

- [ ] **Step 6: Verify request and render deltas**

Capture a populated Recents dashboard with up to eight Stacks and eight Deployments. Expected during 60 seconds: zero `GetStackActionState`, zero `GetDeploymentActionState`, and zero card-owned `GetStack`; summary/list traffic remains at its current fallback cadence. React Profiler shows one row and one card per item before and after resizing across `md`.

- [ ] **Step 7: Commit dashboard reuse separately**

```bash
rtk git add ui/src/resources ui/src/pages/dashboard
rtk git commit -m "perf(ui): reuse dashboard list item data"
```

### Task 7: Centralize query-specific stale and focus policy without slowing fallback polling

**Files:**
- Create: `ui/src/lib/query-policy.ts`
- Modify: `ui/src/app/topbar/alerts.tsx`
- Modify: `ui/src/app/topbar/omni-search/hooks.tsx`
- Modify: `ui/src/resources/dashboard-summary.tsx`
- Modify: `ui/src/resources/read-hooks.ts`
- Modify: `ui/src/resources/deployment/update-available.tsx`
- Modify: `ui/src/resources/stack/update-available.tsx`

- [ ] **Step 1: Add named checkpoint-2 policies**

Implement constants that preserve current fallback intervals but remove focus bursts for already-polled data:

```ts
export const QUERY_POLICY = {
  omni: { refetchInterval: 15_000, staleTime: 15_000, refetchOnWindowFocus: false },
  alertBadge: { refetchInterval: 3_000, staleTime: 3_000, refetchOnWindowFocus: false },
  dashboardSummary: { refetchInterval: 10_000, staleTime: 10_000, refetchOnWindowFocus: false },
  actionState: { refetchInterval: 5_000, staleTime: 5_000, refetchOnWindowFocus: false },
  resourceDetail: { refetchInterval: 30_000, staleTime: 30_000, refetchOnWindowFocus: true },
} as const;
```

Apply only these named policies to their current endpoints. Do not globally change Docker stats, logs, executions, terminal polling, user, version, or queries that are not refreshed by update events.

- [ ] **Step 2: Prove fallback cadence and focus behavior**

With `*/ws/update` blocked, capture three generic and three dashboard samples.
Add `checkpoint2.profile.disconnected` and
`checkpoint2.dashboard.disconnected` groups to the gate spec, plus two
`max-increase` gates against their checkpoint-1 counterparts with
`fraction: 0.05` and `requests: 1`. Run:

```bash
rtk yarn --cwd ui analyze:har-gates -- ../docs/superpowers/evidence/2026-07-10-komodo-ui-network-gates.json
```

Expected: the executable gates pass and per-endpoint cadences match
checkpoint-1 values within timer jitter. With WebSocket connected, focus away
and back five times in 30 seconds; already-polled Omni, alert, and summary
queries must not add focus-only bursts.

- [ ] **Step 3: Commit policy separately**

```bash
rtk git add ui/src/lib/query-policy.ts ui/src/app/topbar ui/src/resources
rtk git commit -m "perf(ui): define query-specific fallback policies"
```

### Task 8: Verify and ship checkpoint 2 before any sequence-dependent work

**Files:**
- Modify: `docs/superpowers/evidence/2026-07-10-komodo-ui-startup-background-traffic.md`
- Modify: `docs/superpowers/evidence/2026-07-10-komodo-ui-network-gates.json`

- [ ] **Step 1: Repeat all four three-sample traffic workloads**

Record raw counts, medians, endpoint breakdown, closed/open Omni delta, compact-card delta, and focus test. Do not claim the 80% synchronized target yet; checkpoint 2 deliberately retains fallback polling.

- [ ] **Step 2: Run checkpoint verification**

```bash
rtk yarn --cwd ui build
rtk yarn --cwd ui analyze:bundle -- --budget 900000
rtk yarn --cwd ui analyze:har-gates -- ../docs/superpowers/evidence/2026-07-10-komodo-ui-network-gates.json
rtk git diff --check main...HEAD
```

Expected: all pass, browser loading/error boundaries still work, and disconnected medians stay within budget.

- [ ] **Step 3: Commit evidence, open the fork-only PR, and keep checkpoint 3 blocked**

```bash
rtk git add docs/superpowers/evidence/2026-07-10-komodo-ui-startup-background-traffic.md docs/superpowers/evidence/2026-07-10-komodo-ui-network-gates.json
rtk git commit -m "docs(ui): record background traffic evidence"
rtk git status --short
rtk git push -u origin ui-background-traffic
rtk gh pr create --repo intezya/komodo --base main --head ui-background-traffic --title "Reduce closed UI background traffic" --body-file docs/superpowers/evidence/2026-07-10-komodo-ui-startup-background-traffic.md
```

Expected: fork-only PR. Do not create `ui-update-stream-sync` until this PR and Runtime Plan 2 Merge Gate B are merged. Rollback is a revert of search, dashboard, or policy commits independently; query keys and stored data did not migrate.

### Task 9: Enforce Merge Gate B and test sequence classification before integration

**Files:**
- Create: `ui/src/lib/update-stream.ts`
- Create: `ui/src/lib/update-stream.test.ts`
- Modify: `ui/package.json`
- Modify: `ui/yarn.lock`
- Verify only: `client/core/ts/src/types.ts`
- Verify only: `client/core/rs/src/entities/update.rs`

- [ ] **Step 1: Stop unless the additive generated contract is present**

```bash
rtk rg -n -U --pcre2 '(?sm)export interface UpdateListItem \{(?:(?!^\}).)*stream_epoch\?: string;(?:(?!^\}).)*^\}' client/core/ts/src/types.ts
rtk rg -n -U --pcre2 '(?sm)export interface UpdateListItem \{(?:(?!^\}).)*sequence\?: number;(?:(?!^\}).)*^\}' client/core/ts/src/types.ts
rtk rg -n -U --pcre2 '(?sm)pub struct UpdateListItem \{(?:(?!^\}).)*pub stream_epoch: Option<String>,(?:(?!^\}).)*^\}' client/core/rs/src/entities/update.rs
rtk rg -n -U --pcre2 '(?sm)pub struct UpdateListItem \{(?:(?!^\}).)*pub sequence: Option<U64>,(?:(?!^\}).)*^\}' client/core/rs/src/entities/update.rs
```

Expected after Merge Gate B: all four independent searches pass;
`UpdateListItem` contains both optional TypeScript fields and the Rust event type
contains both additive serialized counterparts. If any search fails, stop
checkpoint 3; do not add a frontend-only intersection type or manually edit
generated client output.

- [ ] **Step 2: Add the lightweight pure-test harness**

```bash
rtk yarn --cwd ui add --dev --exact tsx@4.23.0
```

Add `"test:update-stream": "tsx --test src/lib/update-stream.test.ts"` to `ui/package.json`. This uses Node's test runner and does not introduce a DOM/component runner.

- [ ] **Step 3: Write the sequence decision tests first**

Define and test these public types:

```ts
export type StreamCursor = { streamEpoch: string; sequence: number };
export type SequenceDecision =
  | { kind: "legacy" }
  | { kind: "malformed"; reason: "partial-metadata" }
  | { kind: "apply"; cursor: StreamCursor }
  | { kind: "ignore"; reason: "duplicate" | "out-of-order" }
  | { kind: "resynchronize"; reason: "epoch-change" | "sequence-gap" };

export function classifyUpdate(
  cursor: StreamCursor | undefined,
  update: Pick<Types.UpdateListItem, "stream_epoch" | "sequence">,
): SequenceDecision;
```

Tests must cover: both metadata fields missing; epoch-only metadata;
sequence-only metadata; first sequenced event at an arbitrary number;
contiguous event; equal duplicate; lower out-of-order event; higher gap; and
changed epoch. Both fields missing returns `legacy`. Exactly one missing returns
`malformed`; it is never applied or replayed and forces a transport reset plus
the normal reconnect barrier. No cursor accepts a partial pair; equal/lower
complete values never apply; gap/epoch return `resynchronize`.

- [ ] **Step 4: Run the tests and see the intended failure**

```bash
rtk yarn --cwd ui test:update-stream
```

Expected now: FAIL because `classifyUpdate` or its module does not exist.

- [ ] **Step 5: Implement the minimum classifier and pass**

Implement only the rules above:

```ts
import type { Types } from "komodo_client";

export type StreamCursor = {
  streamEpoch: string;
  sequence: number;
};

export type SequenceDecision =
  | { kind: "legacy" }
  | { kind: "malformed"; reason: "partial-metadata" }
  | { kind: "apply"; cursor: StreamCursor }
  | { kind: "ignore"; reason: "duplicate" | "out-of-order" }
  | {
      kind: "resynchronize";
      reason: "epoch-change" | "sequence-gap";
    };

export function classifyUpdate(
  cursor: StreamCursor | undefined,
  update: Pick<Types.UpdateListItem, "stream_epoch" | "sequence">,
): SequenceDecision {
  const hasEpoch = update.stream_epoch !== undefined;
  const hasSequence = update.sequence !== undefined;
  if (!hasEpoch && !hasSequence) {
    return { kind: "legacy" };
  }
  if (hasEpoch !== hasSequence) {
    return { kind: "malformed", reason: "partial-metadata" };
  }
  const next = {
    streamEpoch: update.stream_epoch!,
    sequence: update.sequence!,
  };
  if (!cursor) return { kind: "apply", cursor: next };
  if (cursor.streamEpoch !== next.streamEpoch) {
    return { kind: "resynchronize", reason: "epoch-change" };
  }
  if (next.sequence === cursor.sequence) {
    return { kind: "ignore", reason: "duplicate" };
  }
  if (next.sequence < cursor.sequence) {
    return { kind: "ignore", reason: "out-of-order" };
  }
  if (next.sequence !== cursor.sequence + 1) {
    return { kind: "resynchronize", reason: "sequence-gap" };
  }
  return { kind: "apply", cursor: next };
}
```

Then run:

```bash
rtk yarn --cwd ui test:update-stream
```

Expected: all nine classification cases pass, including both partial-metadata
directions as distinct malformed cases.

- [ ] **Step 6: Commit contract and classifier**

```bash
rtk git add ui/package.json ui/yarn.lock ui/src/lib/update-stream.ts ui/src/lib/update-stream.test.ts
rtk git commit -m "test(ui): define update stream sequence rules"
```

### Task 10: Build a deterministic reconnect/refetch coordinator around the classifier

**Files:**
- Create: `ui/src/lib/update-stream-coordinator.ts`
- Create: `ui/src/lib/update-stream-coordinator.test.ts`
- Modify: `ui/package.json`

- [ ] **Step 1: Write coordinator tests before implementation**

First change the script to
`"test:update-stream": "tsx --test src/lib/update-stream.test.ts src/lib/update-stream-coordinator.test.ts"`, then use deferred Promises and fake callbacks to test this exact public API:

```ts
export type StreamPhase =
  | "disconnected"
  | "synchronizing"
  | "synchronized"
  | "legacy"
  | "degraded";

export type SynchronizationReason =
  | "login"
  | "reconnect"
  | "epoch-change"
  | "sequence-gap"
  | "visibility-resume"
  | "retry";

export type TransportResetReason =
  | "malformed-metadata"
  | "queue-overflow";

export type UpdateStreamCallbacks = {
  synchronize: (reason: SynchronizationReason) => Promise<void>;
  apply: (update: Types.UpdateListItem) => void;
  setPhase: (phase: StreamPhase) => void;
  resetTransport: (generation: number, reason: TransportResetReason) => void;
};

export class UpdateStreamCoordinator {
  constructor(callbacks: UpdateStreamCallbacks);
  connect(generation: number, reason: "login" | "reconnect"): Promise<void>;
  disconnect(generation: number): void;
  receive(generation: number, update: Types.UpdateListItem): void;
  retry(generation: number): Promise<void>;
  resynchronize(generation: number, reason: Exclude<SynchronizationReason, "login" | "reconnect" | "retry">): Promise<void>;
  phase(): StreamPhase;
  lastSynchronizedAt(): number | undefined;
}
```

Required tests: login does not report synchronized before the barrier resolves
and, without an event, resolves conservatively to `legacy`; a sequenced event
arriving during the initial barrier is applied only after refetch and enables
`synchronized`; reconnect during a mutation buffers the terminal event;
duplicate and lower events do not call `apply`; gap and epoch change each run
another barrier before applying; a failed barrier enters `degraded`, retains
queued events, and succeeds on `retry`; `disconnect` clears cursor and queued
old-connection events; legacy event applies and remains `legacy`; switching
from legacy to sequenced metadata requires a barrier; visibility
resynchronization updates `lastSynchronizedAt` without upgrading an
unconfirmed/legacy stream. Give every transport a monotonically increasing
generation and prove late login/update/close/retry callbacks from generation
`n` are no-ops after generation `n+1` connects; a reset requested by `n` must
never close `n+1`. Test both partial-metadata directions while a
barrier is pending: each resets transport once, applies nothing, and does not
survive into the next connection. Hold a failed barrier, enqueue exactly 256
complete updates, then send a 257th: it must enter degraded, reset transport
once, clear the queue, and apply none of the old connection's events after the
next `connect`.

Record exact phase sequences for queued `n, n+2` and for a queued epoch-change
suffix. Each stays `synchronizing` through the second barrier and publishes no
transient `synchronized` phase (therefore no 60-second policy) before the final
replay-free barrier completes.

- [ ] **Step 2: Run the coordinator tests and see the intended failure**

```bash
rtk yarn --cwd ui test:update-stream
```

Expected: classifier tests pass and coordinator import/tests fail.

- [ ] **Step 3: Implement one serialized barrier**

Implement `ui/src/lib/update-stream-coordinator.ts` with this complete
serialized state machine:

```ts
import type { Types } from "komodo_client";
import {
  classifyUpdate,
  type StreamCursor,
} from "./update-stream";

export const MAX_QUEUED_UPDATES = 256;

export type StreamPhase =
  | "disconnected"
  | "synchronizing"
  | "synchronized"
  | "legacy"
  | "degraded";

export type SynchronizationReason =
  | "login"
  | "reconnect"
  | "epoch-change"
  | "sequence-gap"
  | "visibility-resume"
  | "retry";

export type TransportResetReason =
  | "malformed-metadata"
  | "queue-overflow";

export type UpdateStreamCallbacks = {
  synchronize: (reason: SynchronizationReason) => Promise<void>;
  apply: (update: Types.UpdateListItem) => void;
  setPhase: (phase: StreamPhase) => void;
  resetTransport: (generation: number, reason: TransportResetReason) => void;
};

type ReplayReason = "epoch-change" | "sequence-gap" | undefined;

export class UpdateStreamCoordinator {
  private currentPhase: StreamPhase = "disconnected";
  private connected = false;
  private awaitingSequence = true;
  private connectionGeneration = 0;
  private cursor: StreamCursor | undefined;
  private queued: Types.UpdateListItem[] = [];
  private synchronizedAt: number | undefined;
  private inFlight:
    | { generation: number; promise: Promise<void> }
    | undefined;

  constructor(private readonly callbacks: UpdateStreamCallbacks) {}

  phase(): StreamPhase {
    return this.currentPhase;
  }

  lastSynchronizedAt(): number | undefined {
    return this.synchronizedAt;
  }

  connect(
    generation: number,
    reason: "login" | "reconnect",
  ): Promise<void> {
    if (generation <= this.connectionGeneration) return Promise.resolve();
    this.connectionGeneration = generation;
    this.connected = true;
    this.awaitingSequence = true;
    this.cursor = undefined;
    this.queued = [];
    this.synchronizedAt = undefined;
    this.inFlight = undefined;
    return this.startSynchronization(reason);
  }

  disconnect(generation: number): void {
    if (!this.isCurrent(generation)) return;
    this.connected = false;
    this.awaitingSequence = true;
    this.cursor = undefined;
    this.queued = [];
    this.synchronizedAt = undefined;
    this.inFlight = undefined;
    this.transition("disconnected");
  }

  receive(generation: number, update: Types.UpdateListItem): void {
    if (!this.isCurrent(generation)) return;
    if (classifyUpdate(undefined, update).kind === "malformed") {
      this.failClosed("malformed-metadata");
      return;
    }
    if (
      this.currentPhase === "synchronizing" ||
      this.currentPhase === "degraded"
    ) {
      this.enqueue(update);
      return;
    }

    const reason = this.process(update);
    if (reason) {
      if (!this.enqueue(update)) return;
      void this.startSynchronization(reason).catch(() => undefined);
    }
  }

  retry(generation: number): Promise<void> {
    if (!this.isCurrent(generation) || this.currentPhase !== "degraded") {
      return Promise.reject(
        new Error("update stream retry requires a degraded connection"),
      );
    }
    return this.startSynchronization("retry");
  }

  resynchronize(
    generation: number,
    reason: Exclude<
      SynchronizationReason,
      "login" | "reconnect" | "retry"
    >,
  ): Promise<void> {
    return this.isCurrent(generation)
      ? this.startSynchronization(reason)
      : Promise.resolve();
  }

  private transition(phase: StreamPhase): void {
    this.currentPhase = phase;
    this.callbacks.setPhase(phase);
  }

  private isCurrent(generation: number): boolean {
    return this.connected && generation === this.connectionGeneration;
  }

  private enqueue(update: Types.UpdateListItem): boolean {
    if (this.queued.length >= MAX_QUEUED_UPDATES) {
      this.failClosed("queue-overflow");
      return false;
    }
    this.queued.push(update);
    return true;
  }

  private failClosed(reason: TransportResetReason): void {
    if (!this.connected) return;
    const generation = this.connectionGeneration;
    this.connected = false;
    this.awaitingSequence = true;
    this.cursor = undefined;
    this.queued = [];
    this.synchronizedAt = undefined;
    this.inFlight = undefined;
    this.transition("degraded");
    this.callbacks.resetTransport(generation, reason);
  }

  private startSynchronization(
    reason: SynchronizationReason,
  ): Promise<void> {
    const generation = this.connectionGeneration;
    if (this.inFlight?.generation === generation) {
      return this.inFlight.promise;
    }
    const promise = this.synchronizationLoop(reason, generation).finally(
      () => {
        if (this.inFlight?.generation === generation) {
          this.inFlight = undefined;
        }
      },
    );
    this.inFlight = { generation, promise };
    return promise;
  }

  private async synchronizationLoop(
    initialReason: SynchronizationReason,
    generation: number,
  ): Promise<void> {
    let reason = initialReason;
    while (this.isCurrent(generation)) {
      this.transition("synchronizing");
      try {
        await this.callbacks.synchronize(reason);
      } catch (error) {
        if (!this.isCurrent(generation)) return;
        this.transition("degraded");
        throw error;
      }
      if (!this.isCurrent(generation)) return;

      this.cursor = undefined;
      const replayReason = this.replayQueued();
      if (!this.isCurrent(generation)) return;
      if (!replayReason) {
        this.synchronizedAt = Date.now();
        this.transition(
          this.awaitingSequence ? "legacy" : "synchronized",
        );
        return;
      }
      reason = replayReason;
    }
  }

  private replayQueued(): ReplayReason {
    const queued = this.queued;
    this.queued = [];
    for (let index = 0; index < queued.length; index += 1) {
      const update = queued[index];
      const reason = this.process(update);
      if (reason) {
        this.queued = queued.slice(index);
        return reason;
      }
    }
    return undefined;
  }

  private process(update: Types.UpdateListItem): ReplayReason {
    const hasSequence =
      update.stream_epoch !== undefined && update.sequence !== undefined;
    if (this.currentPhase === "legacy" && hasSequence) {
      this.awaitingSequence = false;
      return "epoch-change";
    }
    if (hasSequence) this.awaitingSequence = false;

    const decision = classifyUpdate(this.cursor, update);
    switch (decision.kind) {
      case "legacy":
        this.awaitingSequence = true;
        this.cursor = undefined;
        this.transition("legacy");
        this.callbacks.apply(update);
        return undefined;
      case "malformed":
        this.failClosed("malformed-metadata");
        return undefined;
      case "apply":
        this.cursor = decision.cursor;
        this.callbacks.apply(update);
        return undefined;
      case "ignore":
        return undefined;
      case "resynchronize":
        return decision.reason;
    }
  }
}
```

One generation token isolates reconnects from an older in-flight barrier. A
failed barrier leaves its FIFO untouched only up to the fixed 256-event budget;
overflow clears the FIFO and closes the transport, so an outage cannot create
unbounded memory or a replay burst. Partial metadata follows the same fail-closed
reset and is never queued or applied. Replay applies only after a successful
barrier and starts another serialized barrier when its remaining suffix reveals
a gap or epoch change. A login barrier with no Update event ends conservatively
in `legacy`: the old and new Core wire formats are indistinguishable until an
event arrives. The first complete metadata pair runs one more barrier, then
enables `synchronized`; therefore an idle old Core can never accidentally
receive the 60-second policy.

- [ ] **Step 4: Pass all pure tests and commit**

```bash
rtk yarn --cwd ui test:update-stream
rtk git add ui/package.json ui/src/lib/update-stream-coordinator.ts ui/src/lib/update-stream-coordinator.test.ts
rtk git commit -m "feat(ui): coordinate update stream synchronization"
```

Expected: all sequence, reconnect, failure, and compatibility tests pass deterministically.

### Task 11: Integrate the full active-query barrier and WebSocket-first cache update

**Files:**
- Create: `ui/src/lib/websocket-query-families.ts`
- Create: `ui/src/lib/update-stream-state.ts`
- Create: `ui/src/lib/update-transport-lifecycle.ts`
- Create: `ui/src/lib/update-transport-lifecycle.test.ts`
- Modify: `ui/package.json`
- Modify: `ui/src/lib/update-stream.ts`
- Modify: `ui/src/lib/update-stream.test.ts`
- Modify: `ui/src/lib/socket.tsx`
- Modify: `ui/src/app/topbar/websocket-status.tsx`

- [ ] **Step 1: Define the exact active query barrier**

Create the file with these exact imports, query families, and exports:

```ts
import type { Query, QueryClient } from "@tanstack/react-query";
import { Types } from "komodo_client";

export const WS_BARRIER_QUERY_TYPES = new Set<Types.ReadRequest["type"]>([
  "ListUpdates",
  "GetUpdate",
  "ListAlerts",
  "GetSwarmActionState",
  "GetServerActionState",
  "GetStackActionState",
  "GetDeploymentActionState",
  "GetBuildActionState",
  "GetRepoActionState",
  "GetProcedureActionState",
  "GetActionActionState",
  "GetResourceSyncActionState",
  "ListDockerContainers",
  "InspectDockerContainer",
  "ListDockerNetworks",
  "InspectDockerNetwork",
  "ListDockerImages",
  "InspectDockerImage",
  "ListDockerVolumes",
  "InspectDockerVolume",
  "GetResourceMatchingContainer",
  "ListSwarms",
  "ListFullSwarms",
  "GetSwarmsSummary",
  "GetSwarm",
  "ListSwarmNodes",
  "InspectSwarmNode",
  "ListSwarmStacks",
  "InspectSwarmStack",
  "ListSwarmServices",
  "InspectSwarmService",
  "ListSwarmTasks",
  "InspectSwarmTask",
  "ListSwarmConfigs",
  "InspectSwarmConfig",
  "ListSwarmSecrets",
  "InspectSwarmSecret",
  "ListServers",
  "ListFullServers",
  "GetServersSummary",
  "GetServer",
  "GetServerState",
  "GetHistoricalServerStats",
  "ListStacks",
  "ListFullStacks",
  "GetStacksSummary",
  "ListCommonStackExtraArgs",
  "ListComposeProjects",
  "GetStackLog",
  "SearchStackLog",
  "GetStack",
  "ListStackServices",
  "ListDeployments",
  "GetDeploymentsSummary",
  "GetDeployment",
  "GetDeploymentLog",
  "SearchDeploymentLog",
  "GetDeploymentContainer",
  "ListBuilds",
  "ListFullBuilds",
  "GetBuildsSummary",
  "GetBuildMonthlyStats",
  "GetBuild",
  "ListBuildVersions",
  "ListRepos",
  "ListFullRepos",
  "GetReposSummary",
  "GetRepo",
  "ListSchedules",
  "ListProcedures",
  "ListFullProcedures",
  "GetProceduresSummary",
  "GetProcedure",
  "ListActions",
  "ListFullActions",
  "GetActionsSummary",
  "GetAction",
  "ListResourceSyncs",
  "ListFullResourceSyncs",
  "GetResourceSyncsSummary",
  "GetResourceSync",
  "ListBuilders",
  "ListFullBuilders",
  "GetBuildersSummary",
  "GetBuilder",
  "ListAlerters",
  "ListFullAlerters",
  "GetAlertersSummary",
  "GetAlerter",
  "ListVariables",
  "GetVariable",
]);

export const WS_SAFETY_POLL_QUERY_TYPES = new Set<
  Types.ReadRequest["type"]
>([
  "ListUpdates",
  "GetUpdate",
  "GetSwarmActionState",
  "GetServerActionState",
  "GetStackActionState",
  "GetDeploymentActionState",
  "GetBuildActionState",
  "GetRepoActionState",
  "GetProcedureActionState",
  "GetActionActionState",
  "GetResourceSyncActionState",
  "GetSwarmsSummary",
  "GetServersSummary",
  "GetStacksSummary",
  "GetDeploymentsSummary",
  "GetBuildsSummary",
  "GetReposSummary",
  "GetProceduresSummary",
  "GetActionsSummary",
  "GetResourceSyncsSummary",
  "GetBuildersSummary",
  "GetAlertersSummary",
]);

export function isWebsocketBarrierQuery(query: Query): boolean {
  const type = query.queryKey[0];
  return (
    typeof type === "string" &&
    WS_BARRIER_QUERY_TYPES.has(type as Types.ReadRequest["type"])
  );
}

export async function synchronizeWebsocketQueries(
  queryClient: QueryClient,
): Promise<void> {
  await queryClient.invalidateQueries({
    predicate: isWebsocketBarrierQuery,
    refetchType: "none",
  });
  await queryClient.refetchQueries(
    { predicate: isWebsocketBarrierQuery, type: "active" },
    { throwOnError: true },
  );
}
```

The predicate reads only `query.queryKey[0]`; it does not alter any query key. `throwOnError: true` is required so a failed active refetch keeps the stream degraded.
The barrier set is deliberately broader than the safety-poll set: reconnects
must repair every cache family touched by an Update. Update detail, the
unfiltered first Update page, and action-state queries are maintained between
barriers by Update frames. Dashboard summaries are also admitted to the
60-second set as an explicit idle-dashboard freshness tradeoff; they still
poll and are not claimed to be event-authoritative. Live logs, historical
stats, alerts, Docker inspection, Swarm runtime lists, and other
independently changing detail data keep their existing polling cadence.

- [ ] **Step 2: Patch the unfiltered first update page before targeted invalidation**

Add this pure helper to `update-stream.ts` and cover replacement, prepend,
descending order, the 100-item bound, and exact query-key classification in
`update-stream.test.ts`:

```ts
export function upsertUpdateListItem(
  current: Types.UpdateListItem[],
  incoming: Types.UpdateListItem,
): Types.UpdateListItem[] {
  return [
    incoming,
    ...current.filter((update) => update.id !== incoming.id),
  ]
    .sort((left, right) => right.start_ts - left.start_ts)
    .slice(0, 100);
}

export function isUnfilteredFirstUpdatesKey(
  queryKey: readonly unknown[],
): boolean {
  if (queryKey.length !== 2 || queryKey[0] !== "ListUpdates") return false;
  const params = queryKey[1];
  return (
    typeof params === "object" &&
    params !== null &&
    !Array.isArray(params) &&
    Object.keys(params).length === 0
  );
}
```

In the coordinator's `apply` callback, patch the exact unfiltered first-page
key, invalidate every other ListUpdates key, and only then run existing
notification and target-specific invalidation behavior:

```ts
queryClient.setQueryData<Types.ListUpdatesResponse>(
  ["ListUpdates", {}],
  (current) =>
    current
      ? { ...current, updates: upsertUpdateListItem(current.updates, update) }
      : current,
);
void queryClient.invalidateQueries({
  queryKey: ["ListUpdates"],
  predicate: (query) => !isUnfilteredFirstUpdatesKey(query.queryKey),
});
applyRef.current(update);
```

Remove the current unconditional `invalidate(["ListUpdates"])` from
`onUpdate`; leaving it there would immediately refetch the key just patched.
Keep `GetUpdate`, action-state, resource, notification, and attached-message
behavior unchanged. Do not insert the event into filtered or paginated
`ListUpdates` caches because the client cannot prove their filter/page
membership; those keys are invalidated and active observers refetch. Tests must
prove `{}` is excluded while `{ query: {}, page: 0 }`, filtered params, and
other pages are invalidated. The 100-row cap matches Core's first page and
prevents a long-lived socket from growing cache memory. Duplicate/out-of-order
events never reach this callback, so they cannot replace newer cache state or
display duplicate notifications.

- [ ] **Step 3: Wire login, reconnect, retry, close, and visibility**

Create the independent phase atom now (Task 12's central query hook consumes
it later):

```ts
// ui/src/lib/update-stream-state.ts
import { atom } from "jotai";
import type { StreamPhase } from "./update-stream-coordinator";

export const updateStreamPhaseAtom = atom<StreamPhase>("disconnected");
```

Replace `connected: boolean` with transport state plus this atom. The
coordinator's `setPhase` callback writes the atom. On `on_login`, call
`coordinator.connect(count === 0 ? "login" : "reconnect")`; do not show green
before it resolves. On `on_update`, call `coordinator.receive`. On close, call
`disconnect`, preserve the current five-second socket reconnect behavior, and
clear any barrier retry timer. If synchronization fails while the socket
remains open, retry every five seconds with `coordinator.retry()`.

Use this exact state split in `socket.tsx`:

```tsx
const wsAtom = atom<{
  ws: WebSocket | undefined;
  count: number;
}>({ ws: undefined, count: 0 });

export function useWebsocketPhase() {
  return useAtomValue(updateStreamPhaseAtom);
}

export function useWebsocketConnected() {
  return useWebsocketPhase() === "synchronized";
}
```

In `useWebsocketReconnect`, keep the existing close and count increment but
set only `{ ws: undefined, count: state.count + 1 }`. The topbar migrates to
`useWebsocketPhase`; `useWebsocketConnected` stays as a compatibility wrapper
for any non-status consumer.

Create exactly one coordinator for the provider. Import `useQueryClient`,
`useAtomValue`, `useSetAtom`, `UpdateStreamCoordinator`,
`synchronizeWebsocketQueries`, `upsertUpdateListItem`,
`isUnfilteredFirstUpdatesKey`, type-only `TransportResetReason`, and the phase
atom, then place this after `on_update_fn`:

```tsx
  const queryClient = useQueryClient();
  const setStreamPhase = useSetAtom(updateStreamPhaseAtom);
  const applyRef = useRef(on_update_fn);
  applyRef.current = on_update_fn;
  const nextTransportGenerationRef = useRef(0);
  const resetTransportRef = useRef<
    | {
        generation: number;
        reset: (reason: TransportResetReason) => void;
      }
    | undefined
  >(undefined);
  const coordinatorRef = useRef<
    UpdateStreamCoordinator | undefined
  >(undefined);
  if (!coordinatorRef.current) {
    coordinatorRef.current = new UpdateStreamCoordinator({
      synchronize: () => synchronizeWebsocketQueries(queryClient),
      apply: (update) => {
        queryClient.setQueryData<Types.ListUpdatesResponse>(
          ["ListUpdates", {}],
          (current) =>
            current
              ? {
                  ...current,
                  updates: upsertUpdateListItem(current.updates, update),
                }
              : current,
        );
        void queryClient.invalidateQueries({
          queryKey: ["ListUpdates"],
          predicate: (query) =>
            !isUnfilteredFirstUpdatesKey(query.queryKey),
        });
        applyRef.current(update);
      },
      setPhase: setStreamPhase,
      resetTransport: (generation, reason) => {
        const active = resetTransportRef.current;
        if (active?.generation === generation) active.reset(reason);
      },
    });
  }
  const coordinator = coordinatorRef.current;
```

Inside the socket effect, keep one local `barrierRetry` timer and use these
helpers/callback bodies:

```tsx
      let disposed = false;
      const generation = ++nextTransportGenerationRef.current;
      let transportRetry: number | undefined;
      let barrierRetry: number | undefined;
      const clearBarrierRetry = () => {
        window.clearTimeout(barrierRetry);
        barrierRetry = undefined;
      };
      const scheduleBarrierRetry = () => {
        if (disposed) return;
        clearBarrierRetry();
        barrierRetry = window.setTimeout(() => {
          if (disposed) return;
          void coordinator.retry(generation).catch(() => {
            if (!disposed && coordinator.phase() === "degraded") {
              scheduleBarrierRetry();
            }
          });
        }, 5_000);
      };
      const onVisibility = () => {
        if (disposed) return;
        const synchronizedAt = coordinator.lastSynchronizedAt();
        if (
          document.visibilityState === "visible" &&
          synchronizedAt !== undefined &&
          Date.now() - synchronizedAt > 60_000
        ) {
          void coordinator
            .resynchronize(generation, "visibility-resume")
            .catch(scheduleBarrierRetry);
        }
      };
      document.addEventListener("visibilitychange", onVisibility);

      const socket = komodo_client().get_update_websocket({
        on_login: () => {
          if (disposed) return;
          console.info(count, "| Logged into Update websocket");
          void coordinator
            .connect(generation, count === 0 ? "login" : "reconnect")
            .catch(scheduleBarrierRetry);
        },
        on_update: (update) => {
          if (disposed) return;
          coordinator.receive(generation, update);
        },
        on_close: () => {
          if (disposed) return;
          console.info(count, "| Update websocket connection closed");
          clearBarrierRetry();
          coordinator.disconnect(generation);
          if (!disable_reconnect) {
            transportRetry = window.setTimeout(() => {
              if (!disposed && countRef.current === count) {
                console.info(
                  count,
                  "| Automatically triggering reconnect",
                );
                reconnect();
              }
            }, 5_000);
          }
        },
      });
      resetTransportRef.current = {
        generation,
        reset: (reason) => {
          if (disposed) return;
          console.error(`Resetting Update websocket: ${reason}`);
          clearBarrierRetry();
          socket.close(4000, reason);
        },
      };
      setWs((state) => ({ ...state, ws: socket }));

      return () => {
        disposed = true;
        socket.close();
        window.clearTimeout(transportRetry);
        clearBarrierRetry();
        document.removeEventListener(
          "visibilitychange",
          onVisibility,
        );
        if (resetTransportRef.current?.generation === generation) {
          resetTransportRef.current = undefined;
        }
        setWs((state) =>
          state.ws === socket ? { ...state, ws: undefined } : state,
        );
        coordinator.disconnect(generation);
      };
```

Remove `ws.ws` from this effect's dependency array; depend on `user`,
`disable_reconnect`, and `ws.count`. Setting the newly created socket into
`wsAtom` must not immediately run cleanup and disconnect its coordinator.
Every manual/automatic reconnect increments `count`, so it still tears down
the old effect and starts the next transport. Remove the baseline
`ws.ws === undefined` creation guard: the effect generation, not atom contents,
owns creation. When `user` is absent, clear only the currently stored socket
identity and do not increment `count`; cleanup does the same on user or config
transition. Every login, update, close, timer, visibility, retry, and reset
callback checks `disposed`, while the generation parameter makes an already
running old callback a coordinator no-op. Identity-safe atom cleanup and the
generation-tagged reset prevent an old effect from clearing or closing its
replacement.

Do not recreate the coordinator on each render or capture `on_update_fn`
directly in its constructor; the refs keep current
notification/invalidation logic without resetting the stream cursor.

Track `document.visibilitychange`. When the document becomes visible and `Date.now() - lastSynchronizedAt > 60_000`, call `resynchronize("visibility-resume")`; this repairs timers suspended in a background tab before returning to slow cadence.

Extract the effect-owned generation/disposal/reset/identity logic into pure
`createUpdateTransportLifecycle` in `update-transport-lifecycle.ts`. Inject
socket close, timer set/clear, atom get/set, and coordinator operations; the
React effect supplies browser implementations. Its returned guarded
`onLogin`, `onUpdate`, `onClose`, `onVisibility`, `retry`, `reset`, and
`dispose` functions are the only callbacks installed on a socket. `dispose`
clears only its own socket identity/reset generation and timers.

In `update-transport-lifecycle.test.ts`, retain callbacks from socket generation
`n`, connect generation `n+1`, then invoke every retained callback and reset in
turn. Assert the new socket remains open, its atom identity remains installed,
its cursor/queue/phase do not change, and no retry timer is created. Repeat for
a user transition and for toggling `disable_reconnect`; each transition creates
exactly one replacement transport without a `ws.ws` dependency or creation
guard.

Append this test file to the existing `test:update-stream` package script; no
DOM/jsdom is required because timers, visibility, sockets, atoms, and
coordinator calls are injected fakes.

- [ ] **Step 4: Make status represent freshness, not only transport**

Update the topbar indicator: green only for `synchronized`; yellow for
`synchronizing` and `legacy`; red for `disconnected` and `degraded`. The hover
text must distinguish `Synchronizing`, `Connected`, `Connected; awaiting a
sequenced event or using an old Core; fallback polling active`,
`Synchronization failed; fallback polling active`, and `Disconnected`.
Clicking still forces reconnect.

- [ ] **Step 5: Test and build**

```bash
rtk yarn --cwd ui test:update-stream
rtk yarn --cwd ui build
rtk awk '/invalidate\(\["ListUpdates"\]\)/ { found = 1 } END { exit found }' ui/src/lib/socket.tsx
```

Expected: tests and build pass, partial metadata and queue overflow reset without
apply/replay, key tests preserve the patched unfiltered first page, and the
static guard proves the old unconditional ListUpdates invalidation is gone. No
UI state can report synchronized before all active managed queries refetch
successfully.

- [ ] **Step 6: Commit integration separately**

```bash
rtk git add ui/package.json ui/src/lib/socket.tsx ui/src/lib/websocket-query-families.ts ui/src/lib/update-stream-state.ts ui/src/lib/update-stream.ts ui/src/lib/update-stream.test.ts ui/src/lib/update-transport-lifecycle.ts ui/src/lib/update-transport-lifecycle.test.ts ui/src/app/topbar/websocket-status.tsx
rtk git commit -m "feat(ui): synchronize queries on update stream reconnect"
```

### Task 12: Switch the reviewed slow-poll observers to 60-second safety polling

**Files:**
- Modify: `ui/src/lib/query-policy.ts`
- Modify: `ui/src/lib/hooks.ts`
- Modify: `ui/src/lib/update-stream.test.ts`

- [ ] **Step 1: Test policy as a pure function first**

Add cases to `update-stream.test.ts` for this API:

```ts
export type LiveQueryPolicy = {
  refetchInterval: number | false;
  staleTime: number;
  refetchOnWindowFocus: boolean;
};

export function liveQueryPolicy(
  phase: StreamPhase,
  fallback: LiveQueryPolicy,
): LiveQueryPolicy;
```

Expected rules: only `synchronized` returns `refetchInterval: 60_000`,
`staleTime: 60_000`, and `refetchOnWindowFocus: false`. `disconnected`,
`synchronizing`, `legacy`, and `degraded` return the supplied fallback object
unchanged, including its checkpoint-2 `staleTime`.

Add a classification regression asserting `ListUpdates`,
`GetServerActionState`, and `GetServersSummary` are in both sets, while `ListAlerts`,
`GetHistoricalServerStats`, `GetStackLog`, and `GetDeploymentLog` are in
`WS_BARRIER_QUERY_TYPES` but not `WS_SAFETY_POLL_QUERY_TYPES`. This prevents a
future broad-set reuse from silently slowing live data.

- [ ] **Step 2: Add the hook wrapper and migrate only managed queries**

Retain `QUERY_POLICY` from checkpoint 2 and add the type import, type, and pure
function exactly as:

```ts
import type { StreamPhase } from "./update-stream-coordinator";

export type LiveQueryPolicy = {
  refetchInterval: number | false;
  staleTime: number;
  refetchOnWindowFocus: boolean;
};

export function liveQueryPolicy(
  phase: StreamPhase,
  fallback: LiveQueryPolicy,
): LiveQueryPolicy {
  return phase === "synchronized"
    ? {
        refetchInterval: 60_000,
        staleTime: 60_000,
        refetchOnWindowFocus: false,
      }
    : fallback;
}
```

The socket coordinator's `setPhase` callback already writes the independent
atom created in Task 11; `hooks.ts` therefore never imports `socket.tsx`. In
`useRead`, import `useAtomValue`,
`updateStreamPhaseAtom`, `WS_SAFETY_POLL_QUERY_TYPES`, `liveQueryPolicy`, and the
type-only `LiveQueryPolicy`, then
add this policy derivation before `useQuery`:

```ts
  const phase = useAtomValue(updateStreamPhaseAtom);
  const configuredInterval = config?.refetchInterval;
  const configuredStaleTime = config?.staleTime;
  const configuredFocus = config?.refetchOnWindowFocus;
  const fallback: LiveQueryPolicy = {
    refetchInterval:
      typeof configuredInterval === "number" ||
      configuredInterval === false
        ? configuredInterval
        : false,
    staleTime:
      typeof configuredStaleTime === "number" ? configuredStaleTime : 0,
    refetchOnWindowFocus:
      typeof configuredFocus === "boolean"
        ? configuredFocus
        : true,
  };
  const streamPolicy = WS_SAFETY_POLL_QUERY_TYPES.has(type)
    ? liveQueryPolicy(phase, fallback)
    : undefined;
```

Spread `...streamPolicy` after `...config` in the existing `useQuery` options.
This applies the safety cadence to every active reviewed slow-poll observer—including
direct `useRead` calls outside `read-hooks`—without changing query keys. The
checkpoint-2 policies remain their exact fallback objects.

Keep Omni's `ListAllDockerContainers` and `ListTerminals` on their enabled-only
15-second policy: neither query is invalidated by Update events, so both are
absent from `WS_SAFETY_POLL_QUERY_TYPES` and the central hook leaves them alone.
Likewise, system stats, terminal lists, `GetUser`, and `GetVersion` remain
unchanged because they are not in the set. `ListAlerts`,
`GetHistoricalServerStats`, `GetStackLog`, `SearchStackLog`,
`GetDeploymentLog`, `SearchDeploymentLog`, Docker/Swarm runtime queries, and
all other barrier-only families retain their checkpoint-2 cadence because
their data can change without an Update frame.

Dashboard summary queries are the only independently changing family in
`WS_SAFETY_POLL_QUERY_TYPES`. Their connected idle cadence intentionally moves
from 10 seconds to 60 seconds to meet the approved dashboard request budget;
disconnected, legacy, synchronizing, and degraded phases retain 10 seconds.

- [ ] **Step 3: Prove old Core and disconnected compatibility**

With `*/ws/update` blocked, verify the exact checkpoint-2 intervals. Against an old Core payload with no sequence fields, verify phase `legacy`, notifications still apply, and the same fallback intervals continue. Against new Core, verify the indicator becomes green only after the barrier, every safety-poll observer uses 60 seconds, and each barrier-only live query retains its prior interval.

- [ ] **Step 4: Commit the cadence switch as the rollback unit**

```bash
rtk yarn --cwd ui test:update-stream
rtk yarn --cwd ui build
rtk git add ui/src/lib/query-policy.ts ui/src/lib/hooks.ts ui/src/lib/update-stream.test.ts
rtk git commit -m "perf(ui): use websocket-first safety polling"
```

Expected: this one commit can be reverted to restore checkpoint-2 cadence without reverting sequence handling.

### Task 13: Verify the failure matrix, 80% budget, rollout, and rollback

**Files:**
- Create: `ui/scripts/update-stream-fault-proxy.mjs`
- Create: `ui/scripts/update-stream-fault-proxy.test.mjs`
- Modify: `ui/package.json`
- Modify: `ui/yarn.lock`
- Modify: `docs/superpowers/evidence/2026-07-10-komodo-ui-startup-background-traffic.md`
- Modify: `docs/superpowers/evidence/2026-07-10-komodo-ui-network-gates.json`

- [ ] **Step 1: Run the deterministic suite and production build**

```bash
rtk yarn --cwd ui test:update-stream
rtk yarn --cwd ui build
rtk yarn --cwd ui analyze:bundle -- --budget 900000
rtk git diff --check main...HEAD
```

Expected: all pass; heavy features remain absent from the initial dashboard graph.

- [ ] **Step 2: Add the deterministic loopback HTTP/WebSocket fault proxy**

Add exact dev dependencies `http-proxy@1.18.1` and `ws@8.18.3`, plus script
`"fault-proxy": "node scripts/update-stream-fault-proxy.mjs"`. Implement the
checked-in proxy with Node `http`, `http-proxy`, and `ws`; it must bind only
`127.0.0.1`, forward HTTP and the `/ws/update` upgrade to the required
`P1_PROXY_TARGET`, preserve Origin/Cookie/Authorization/API-key headers without
logging their values, and support TLS targets. Run the UI locally with
`VITE_KOMODO_HOST` set to the loopback proxy. Use only an isolated QA user and
Core; the proxy must refuse a target whose hostname is not present in the
comma-separated `P1_PROXY_ALLOWED_HOSTS` allowlist.

Reserve `POST /__komodo_faults` locally, require
`Authorization: Bearer $P1_PROXY_CONTROL_TOKEN`, and expose
`GET /__komodo_faults/status` with counters but no request headers or payload
secrets. Implement these deterministic modes, each with a monotonic command id
and JSONL evidence record: `drop-next-update`, `duplicate-next-update`,
`replay-previous-after-next`, connection-scoped `translate-epoch`,
`strip-next-epoch`, `strip-next-sequence`, persistent `strip-all-metadata`,
`burst-complete-updates` with explicit count (257 in the matrix), and
`fail-next-read` with an exact `/read/<Operation>` path. One-shot modes disarm
after exactly one match; persistent mode requires an explicit `clear` command.
`replay-previous-after-next` retains the last already-forwarded raw frame,
forwards the next real frame normally, then replays that retained lower frame;
it never withholds/reorders the real pair. `translate-epoch` assigns one new
epoch to every subsequent frame on that WS connection until the connection is
closed, so it cannot flip back on the following real frame; clearing it closes
that proxied socket and reconnect starts clean. Do not offer one-shot sequence
rewrites or one-shot epoch replacement because they create collisions or a
second artificial epoch change. For a burst, clone one complete authorized frame and assign strictly increasing
sequences in the same epoch; queue overflow must close the connection before
the next real frame, so generated sequences cannot collide. Never synthesize resource data. The proxy must
buffer at most two ordinary frames and 257 burst frames and exit if those bounds
are exceeded.

Use commands of this exact form and record the returned command id before each
case:

```bash
export P1_PROXY_TARGET=https://qa.example.invalid
export P1_PROXY_ALLOWED_HOSTS=qa.example.invalid
export P1_PROXY_CONTROL_TOKEN="$(rtk openssl rand -hex 24)"
rtk yarn --cwd ui fault-proxy -- --listen 127.0.0.1:5180
rtk curl --fail-with-body -H "Authorization: Bearer $P1_PROXY_CONTROL_TOKEN" -H 'Content-Type: application/json' -d '{"mode":"drop-next-update"}' http://127.0.0.1:5180/__komodo_faults
rtk curl --fail-with-body -H "Authorization: Bearer $P1_PROXY_CONTROL_TOKEN" -H 'Content-Type: application/json' -d '{"mode":"translate-epoch"}' http://127.0.0.1:5180/__komodo_faults
rtk curl --fail-with-body -H "Authorization: Bearer $P1_PROXY_CONTROL_TOKEN" -H 'Content-Type: application/json' -d '{"mode":"fail-next-read","path":"/read/ListUpdates"}' http://127.0.0.1:5180/__komodo_faults
rtk curl --fail-with-body -H "Authorization: Bearer $P1_PROXY_CONTROL_TOKEN" -H 'Content-Type: application/json' -d '{"mode":"burst-complete-updates","count":257}' http://127.0.0.1:5180/__komodo_faults
rtk curl --fail-with-body -H "Authorization: Bearer $P1_PROXY_CONTROL_TOKEN" -H 'Content-Type: application/json' -d '{"mode":"clear"}' http://127.0.0.1:5180/__komodo_faults
```

Add a Node integration test with a local fake HTTP/WS upstream. It must prove
header forwarding without log leakage, every mode's exact frames/status and
connection reset semantics,
one-shot disarming, persistent old-Core mode, bounded buffers, target allowlist,
control-token rejection, no collateral loss/collision/extra barrier after each
one-shot, and clean shutdown. Add it to `test:update-stream` and
run it before browser work. Preserve all existing `tsx --test` inputs and
append `scripts/update-stream-fault-proxy.test.mjs` to that package script.

```bash
rtk yarn --cwd ui test:update-stream
rtk git add ui/package.json ui/yarn.lock ui/scripts/update-stream-fault-proxy.mjs ui/scripts/update-stream-fault-proxy.test.mjs
rtk git commit -m "test(ui): add update stream fault proxy"
```

- [ ] **Step 3: Run the complete browser failure matrix**

Using the isolated QA host through the checked-in fault proxy, record these
cases and their proxy command ids in the evidence file:

1. Normal first login: synchronizing indicator, active-query barrier, then
   conservative yellow fallback until the first sequenced event; that event
   triggers a second barrier, then green and 60-second cadence. Establish this
   state before the synchronized traffic captures.
2. Missed event: arm `drop-next-update`, then deliver the next visible frame;
   the following frame exposes the gap and no mutation from it occurs before a
   full barrier. Do not rewrite sequence numbers.
3. Reconnect during a mutation: disconnect after start, reconnect before terminal event; barrier completes, queued terminal event applies once, final state is correct.
4. Duplicate and out-of-order: arm `duplicate-next-update`, then
   `replay-previous-after-next`; the equal/lower replay causes no notification,
   cache replacement, or refetch barrier, and both real frames were already
   delivered in order.
5. Epoch change: arm connection-scoped `translate-epoch`; exactly one barrier
   runs, the cursor resets before apply, and subsequent frames in that
   connection keep the translated epoch with no artificial flip-back. Clear by
   reconnecting before the next case.
6. Failed barrier: arm `fail-next-read` for one active managed operation;
   indicator is degraded, fallback polling remains, queued event is not
   applied, and the five-second retry eventually synchronizes.
7. Malformed metadata: arm `strip-next-epoch`, then `strip-next-sequence`; each applies
   no cache/notification change, resets transport, and completes a new
   authoritative reconnect barrier.
8. Queue overflow: hold the barrier in failure and arm
   `burst-complete-updates` with 257; prove the 257th complete frame
   and prove the 257th resets transport with no replay from the abandoned
   connection.
9. Old Core: arm persistent `strip-all-metadata`; UI enters legacy and retains
   fallback polling, then `clear` before the next case.
10. Background suspension: keep the tab hidden longer than 60 seconds, trigger an update, resume; visibility barrier completes before green/slow mode resumes.
11. On-demand features: search, editor, chart, and terminal still load and work; first-load and failure boundaries remain visible.

- [ ] **Step 4: Run the six three-sample traffic groups and numeric gates**

Use Task 1's exact host/profile, marker-validated 60-second window, and HAR
parser for the four new-Core workloads plus old-Core `/profile` and dashboard.
Add these exact gates to the spec:

- `max-fraction` on metric `owned`: checkpoint-3 profile connected against
  checkpoint-1 profile connected with `fraction: 0.2`;
- `max-fraction` on metric `owned`: checkpoint-3 dashboard connected against
  checkpoint-1 dashboard connected with `fraction: 0.2`;
- `owned-budget-total` on metric `total`: checkpoint-3 dashboard connected
  against checkpoint-1 dashboard connected with `fraction: 0.2`; its limit is
  the frozen untouched median plus 20% of the frozen owned median, not 20% of
  all traffic;
- `absolute-delta` on metric `total`: checkpoint-3 profile/dashboard disconnected against the
  matching checkpoint-2 disconnected group with `fraction: 0.05`,
  `requests: 1`;
- `absolute-delta` on metric `total`: checkpoint-3 old-Core profile/dashboard against the
  matching checkpoint-2 disconnected group with `fraction: 0.05`,
  `requests: 1`.

Run:

```bash
rtk yarn --cwd ui analyze:har-gates -- ../docs/superpowers/evidence/2026-07-10-komodo-ui-network-gates.json
```

Expected: all six new groups contain exactly three marker-valid HARs and every
gate exits 0. If an owned-family 80% target fails, use `byEndpoint` evidence to
fix only the remaining Plan-3-owned observer and repeat. If the separate total
budget fails because untouched traffic increased, diagnose that regression;
never reclassify an endpoint, suppress correctness refetches, or broaden into
unrelated polling to make a percentage pass.

- [ ] **Step 5: Capture render evidence**

React Profiler must show one Spotlight root and no responsive duplicate rows/cards. Record commit, viewport, resource counts, commit count, and render duration for one dashboard mount and one `md` breakpoint transition. This is a regression guard, not a promised render-speed percentage.

- [ ] **Step 6: Open the fork-only checkpoint-3 PR**

```bash
rtk git add docs/superpowers/evidence/2026-07-10-komodo-ui-startup-background-traffic.md docs/superpowers/evidence/2026-07-10-komodo-ui-network-gates.json
rtk git commit -m "docs(ui): record websocket-first performance evidence"
rtk git push -u origin ui-update-stream-sync
rtk gh pr create --repo intezya/komodo --base main --head ui-update-stream-sync --title "Synchronize WebSocket-first UI queries" --body-file docs/superpowers/evidence/2026-07-10-komodo-ui-startup-background-traffic.md
```

Expected: PR base/head remain inside `intezya/komodo`; the PR links Runtime Plan 2 Merge Gate B and lists all failure-matrix evidence.

- [ ] **Step 7: Roll out and retain explicit rollback boundaries**

Deploy checkpoint 3 to QA first. Observe browser error reports, WebSocket reconnect rate, barrier failures, managed HTTP request count, and stale-data reports for at least one normal operating window before production. A frontend rollback is safe with the new Core because fields are optional and old UI ignores them. Roll back in this order:

1. Revert `perf(ui): use websocket-first safety polling` to restore checkpoint-2 intervals while keeping sequence guards.
2. If synchronization itself is faulty, revert `feat(ui): synchronize queries on update stream reconnect` and the coordinator commits.
3. Keep checkpoints 1–2 unless their own bundle/browser regressions are implicated; they do not depend on Merge Gate B.

No rollback requires data migration or query-key invalidation. Do not remove the 60-second safety poll while keeping WebSocket-first mode.

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-10-komodo-ui-startup-background-traffic.md`. Two execution options:

1. **Subagent-driven (recommended):** use `superpowers:subagent-driven-development`, one fresh worker per task with review between tasks.
2. **Sequential session:** use `superpowers:executing-plans` in a dedicated worktree and execute checkpoints in order, stopping at Merge Gate B before checkpoint 3.
