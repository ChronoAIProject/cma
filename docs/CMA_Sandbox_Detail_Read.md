# Sandbox detail snapshot reads

`GET /api/v1/sandboxes/{id}` retains the same authorization, hierarchy projection,
optional enrichment, response, error concealment, and final revocation fence.
The independent controls below change how consistent Mongo reads are held and
how optional presentation reads are scheduled. Record validation remains intact;
Agent projection is a separate, gated change.

## Reader and diagnostic controls

- `CHRONO_ACCESS_SNAPSHOT_SESSION_ENABLED`: use a fresh Mongo snapshot session
  for each individual authorization snapshot, removing its abort command. Defaults
  to `false`; accepts only exact `true` or `false`. Initial authorization and the
  final release fence remain separate snapshots, including failure paths.
- `CHRONO_SANDBOX_DETAIL_ENRICHMENT_CONCURRENT_ENABLED`: issue independent color
  and public-name enrichment reads together after authorization succeeds. Defaults
  to `false`; accepts only exact `true` or `false`. Their individual timers and
  empty-data fallback remain; no shared `ClientSession` is used concurrently.
- `CHRONO_SANDBOX_SNAPSHOT_READS_ENABLED`: use a Mongo snapshot session for
  Sandbox hierarchy reads. Defaults to `false` in configuration and the API
  manifest. Only exact `true` and `false` are accepted.
- `CHRONO_SANDBOX_DETAIL_TIMINGS_ENABLED`: emit bounded phase events; defaults
  to `true` independently of the reader flag. Sandbox and Workspace authorization,
  hierarchy and final-fence phases retain route templates and request hashes.
  Sandbox colors/names enrichment also has phase timers. Workspace detail has
  no colors/names enrichment. See [Mongo diagnostics](CMA_Mongo_Diagnostics.md).

A snapshot session pins the first response's `atClusterTime` across Sandbox,
Workspace, Worktree and Agent reads and cursor batches. Its four collections
explicitly use snapshot read concern; the Mongo driver would otherwise preserve
an inherited majority read concern. It sends no transaction start, commit or
abort. MongoDB 5.0+ is required; the repository's pinned Mongo 8 replica-set
harness exercises this contract. Failures remain explicit unavailable/corrupt
errors; there is no fallback to independent, inconsistent reads.

With the reader flag false, the original transaction and abort path remains.
Workspace hierarchy reads continue using that path in either mode. All existing
query filters, sort order, server deadlines, record readers, retry bounds and
projection checks remain. The final current-authority fence still runs after
hierarchy reading/projection, including when it failed, before the result is
released. This change does not make that fence reuse the older hierarchy snapshot.

## Phase events

The `sandbox_read_phase` tracing target emits only `phase`, `outcome` and numeric
`elapsed_us`, with a static message and no parent span. It accepts no IDs,
credentials, names, documents, or error strings. Phase names are an enum:

| Phase | Boundary |
| --- | --- |
| `auth` | Resource authorization, including the existing timeout; begins after HTTP credential extraction. |
| `hierarchy` | Store read, decoding, hierarchy validation and projection. |
| `final_fence` | Fresh authorization validation before releasing the projected result. |
| `colors` | Existing optional owner color enrichment in the detail handler. |
| `names` | Existing optional public-name enrichment for personal detail views. |

`completed` means the phase returned, `error` means a fallible phase returned an
error, and `cancelled` means its future was dropped. Optional enrichments retain
their existing empty-data fallback, so `completed` does not assert dependency
availability. Existing enrichment warning events remain. Unreached phases are
absent; shared views do not resolve personal owner names. Core phase events also
cover internal callers of the same Sandbox hierarchy service (including owner
inventory); analyze detail requests in an isolated sample rather than treating
all core events as HTTP detail requests. Legacy test compositions without a
resource-access service have no authorization or final-fence phases.

## Validation and activation

The real-Mongo tests under `resource_access_tests::sandbox_snapshot` compare both
modes, assert exact snapshot concern/time/session and checkout counts, preserve
Workspace behavior, cross cursor batches, race committed child moves/deletes,
and refuse corrupt/missing/deadline cases. The existing buffered hierarchy
revocation/expiry/dependency-failure/timeout matrix runs with timings enabled.
Timer tests cover opt-in, all labels, errors, cancellation and parent-span privacy.

The local measurement test performs warmed, authorized reads in both modes and
prints aggregate wall times and command checkout counts alongside phase timings.
It does not simulate production pool contention or identity-provider latency.
An eliminated abort is one checkout saved per Sandbox hierarchy read, not proof
that production's multi-second tail is eliminated.

Before activation, the release owner should capture the current-reader phase
baseline, enable snapshot reads on a controlled cohort, and compare hierarchy
result/error parity, phase latency, pool checkout pressure and revocation tests.
Disable the reader flag to return to the transaction path; disable timing
independently when the measurement window ends. There is no schema migration,
backfill, writer protocol change, or rollback data repair.

## Local comparison and proposed next step

A pinned Mongo 8.0.13 replica-set fixture, warmed with one Sandbox and one child
of each kind, ran forty authorized reads per mode including actual color/name
enrichment. Local p50 was **9.059 → 7.838 ms**, p95 **11.822 → 8.746 ms**;
CMAP checkouts were **13 → 12 per read** (isolated hierarchy **5 → 4**).
This excludes HTTP credential extraction, transport/serialization and production
contention; timing changes include local noise. The checkout reduction is exact.

| Phase | Off p50 / p95 (ms) | On p50 / p95 (ms) |
| --- | ---: | ---: |
| auth | 2.041 / 3.045 | 1.958 / 2.308 |
| hierarchy | 3.413 / 4.410 | 2.608 / 3.073 |
| final_fence | 2.222 / 3.436 | 1.984 / 2.470 |
| colors | 0.492 / 0.793 | 0.499 / 0.594 |
| names | 0.538 / 0.871 | 0.531 / 0.642 |

The existing default-off batching implementation is described in
[Sandbox hierarchy batching](CMA_Sandbox_Hierarchy_Batching.md). Its activation
remains subject to #1743 attribution. The authorization-session and enrichment
controls can be measured and rolled back independently of those hierarchy flags.
Full Agent records and `validate_conversation()` remain in this change. Projected
Agent summaries are kept on a separate branch until the production phase baseline
and attribution support a reviewed scope decision.

## Authorization and enrichment comparison (#1797)

The paired local run on 2026-11-13 used the same candidate binary with each
switch combination, MongoDB 8.0.13 from the repository's digest-pinned image,
one request at a time, and 20 warmed samples per mode and population. The order
rotated each sample. The existing batching fixtures supplied 4/16, 16/256 and
128/4,096 Workspaces/Agents, plus 1,025 unrelated Agents in the same collections.
This is a debug build on a shared development host with a local container limited
to two CPUs and 2 GiB; it excludes HTTP extraction, serialization and production
pool contention. The harness calls the authorized hierarchy service and real
color/name stores; HTTP scheduling and response parity are covered separately.

`A` is the new authorization-session switch, `E` is the new enrichment switch,
and `H` means both existing hierarchy switches. Every deployment default remains
off. `H` was enabled only in disposable local fixtures, pending #1743 attribution.

| Population | Enabled switches | Commands / checkouts per read | getMore per read | p50 / p95 (ms) |
| --- | --- | ---: | ---: | ---: |
| 4 / 16 | none | 13 | 0 | 17.196 / 19.253 |
| 4 / 16 | A | 11 | 0 | 15.561 / 17.401 |
| 4 / 16 | E | 13 | 0 | 16.261 / 18.122 |
| 4 / 16 | A + E | 11 | 0 | 14.964 / 16.189 |
| 4 / 16 | H | 10 | 0 | 14.650 / 17.157 |
| 4 / 16 | A + E + H | 8 | 0 | 12.930 / 14.359 |
| 16 / 256 | none | 15 | 2 | 82.814 / 94.109 |
| 16 / 256 | A | 13 | 2 | 79.897 / 91.727 |
| 16 / 256 | E | 15 | 2 | 80.698 / 83.644 |
| 16 / 256 | A + E | 13 | 2 | 79.225 / 84.300 |
| 16 / 256 | H | 11 | 1 | 79.567 / 84.795 |
| 16 / 256 | A + E + H | 9 | 1 | 77.345 / 81.847 |
| 128 / 4,096 | none | 16 | 3 | 1051.513 / 1085.483 |
| 128 / 4,096 | A | 14 | 3 | 1049.037 / 1089.992 |
| 128 / 4,096 | E | 16 | 3 | 1053.795 / 1090.736 |
| 128 / 4,096 | A + E | 14 | 3 | 1039.713 / 1185.582 |
| 128 / 4,096 | H | 11 | 1 | 1095.110 / 1212.897 |
| 128 / 4,096 | A + E + H | 9 | 1 | 1101.221 / 1167.150 |

The exact reduction with `A` is one `abortTransaction` in `auth` and one in
`final_fence`. Each retains its own lineage aggregate and `resource_shares` find.
With hierarchy flags off, hierarchy remains four finds plus its abort and any
cursor batches. Colors and names each retain one find; `E` overlaps their waits.
With `H`, hierarchy uses one Sandbox find and one tagged child aggregate plus
cursor batches. The measurement output groups commands by phase, collection and
command, and asserts these totals on every sample.

The returned BSON document totals are unchanged by `A` and `E`: 47,291, 625,658
and 9,903,481 bytes per read, respectively. Hierarchy Agent documents contribute
29,360, 472,768 and 7,681,088 bytes for exactly 16, 256 and 4,096 Agents in every
mode. `H` adds tagged-envelope bytes (total 48,363, 641,274 and 10,149,241 bytes),
without reducing Agent document bytes. These are returned BSON document bytes,
not network-frame sizes or server docs-examined counts. No measured read queried
Agent logs; the full-log-scan count is zero.

| Population | Phase | All off p50 / p95 (ms) | A + E p50 / p95 (ms) |
| --- | --- | ---: | ---: |
| 4 / 16 | auth | 2.948 / 3.849 | 2.174 / 2.603 |
| 4 / 16 | hierarchy | 9.031 / 10.635 | 9.228 / 10.063 |
| 4 / 16 | final_fence | 3.029 / 3.591 | 2.322 / 2.639 |
| 4 / 16 | colors | 0.845 / 0.999 | 1.030 / 1.179 |
| 4 / 16 | names | 0.837 / 1.045 | 0.850 / 1.076 |
| 16 / 256 | auth | 3.563 / 4.046 | 2.546 / 2.880 |
| 16 / 256 | hierarchy | 73.079 / 76.070 | 72.844 / 76.885 |
| 16 / 256 | final_fence | 3.750 / 5.223 | 2.831 / 3.119 |
| 16 / 256 | colors | 1.034 / 1.269 | 1.157 / 1.511 |
| 16 / 256 | names | 1.019 / 1.255 | 1.083 / 1.284 |
| 128 / 4,096 | auth | 3.762 / 4.510 | 2.712 / 3.562 |
| 128 / 4,096 | hierarchy | 1040.571 / 1074.601 | 1032.930 / 1177.734 |
| 128 / 4,096 | final_fence | 4.314 / 6.321 | 3.195 / 12.914 |
| 128 / 4,096 | colors | 1.048 / 1.321 | 1.206 / 1.663 |
| 128 / 4,096 | names | 1.030 / 1.468 | 1.178 / 1.446 |

These samples establish command reduction and unchanged returned data. They do
not establish a large-population latency improvement: its `A + E` p95 increased
in this run, while full Agent reading/validation dominated the hierarchy phase.
The production phase baseline, actual Agent population and #1743 attribution
remain prerequisites for projection inclusion and hierarchy activation. Local
debug timings cannot attribute the reported production stalls or pool waits.

Reproduce with the ignored `sandbox_snapshot::access_measurement` contracts in
the repository's disposable Mongo harness. The output has `ACCESS_SAMPLE`,
`ACCESS_COMMANDS`, `ACCESS_MEASURE` and the existing `sandbox_read_phase` events.
Separate real-Mongo contracts verify both authorization modes, shared snapshot
times, retry/failure semantics, and 1,000 owner changes between read and release.
