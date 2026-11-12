# Sandbox detail snapshot reads

`GET /api/v1/sandboxes/{id}` retains the same authorization, hierarchy projection,
optional enrichment, response, error concealment, and final revocation fence.
The reader controls change how Mongo authorization and hierarchy readers hold
their consistent views. Complete record validation remains intact.

## Reader and diagnostic controls

- `CHRONO_ACCESS_SNAPSHOT_SESSION_ENABLED`: use a fresh Mongo snapshot session
  for each individual authorization snapshot, removing its abort command. Defaults
  to `false`; accepts only exact `true` or `false`. Initial authorization and the
  final release fence remain separate snapshots, including failure paths.
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

The hierarchy remains the largest individual measured phase. Propose a separate
default-off streaming aggregation of its three child collections, after the root
read, with indexed filters and explicit per-type ordering. A tagged `$unionWith`
cursor can retain individual document-size limits rather than building one large
joined BSON row. This could save two more checkouts on the small fixture; it is
not yet implemented or benchmarked. Require snapshot/cursor, corruption,
concurrent-mutation, ordering, index and final-revocation parity before choosing
it. Keep initial authorization and the fresh final fence separate. Never run
parallel operations on the same ClientSession.
