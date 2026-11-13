# Staged fleet activation for bounded Agent logs

Refs #1648, #1591, #1634 and #1320. This adds selection and preparation scheduling
to the existing trusted legacy-log implementation. It changes no metadata format,
proof rule, semantic checkpoint, exact receipt, quarantine rule or segmented
writer. The compatible quarantine-aware image remains the rollback floor.

## Selection and rollout

`CHRONO_AGENT_LOG_FLEET_PERCENT` is a canonical integer from 0 to 100, default 0
in code and the API ConfigMap. Positive percentages are mutually exclusive with
`CHRONO_AGENT_LOG_ADMISSION_AGENTS`; the existing explicit mode still accepts at
most 16 IDs and keeps its startup preparation contract. Fleet mode does not
list Agents or prepare a cohort at startup. It supports all current and future
Agents without an ID list.

The stable bucket is the first eight SHA-256 bytes, interpreted big-endian,
modulo 100, over `cma.agent-log.fleet.v1\0` followed by the authoritative Agent
ID bytes. Select buckets strictly below the configured percentage. Selection
is monotonic across 10%, 50% and 100%, deterministic across restarts and independent
of identity enumeration order. A percentage is an approximate population fraction,
not an exact quota; record the observed eligible sample count at each stage.

Deploy at 0, qualify the compatible image and the owner's explicit test cohort,
then request individual owner approval for fleet stages 10 -> 50 -> 100. Fleet
percentages can include other users and require that exposure in the approval. Configuration is loaded from environment, so every stage requires a
same-image API rollout and confirmation that all old processes have drained.
No existing dynamic control channel is introduced or extended. Keep the explicit
cohort empty in fleet mode. The kill switch is percentage 0 plus an empty explicit
cohort, rolled out on the same compatible image. It disables both admission and
public-window selection. Independent title/other reader switches remain separate.

Verify effective Deployment environment overrides as well as the ConfigMap:
an explicit ID override must not survive a positive percentage rollout. Shared
selection activates admission and public-window reads together. Qualify admission
first, then open-Agent reads before expanding each stage, and report their scan
and latency effects separately rather than attributing the joint change to one.

Disabling selection does not disable quarantine writes by explicit audits or
full writer fallbacks. Keep every marker, event and receipt. Never remove a marker
to authorize an old image. An older-image rollback needs its own compatibility
qualification; percentage 0 is not evidence that the old binary is safe.

### Checked-in stage controls

`.github/agent-log-stages/{off,10,50,100}.json` are explicit ConfigMap merge
patches for the release controller. They are not included by the base deployment;
the base ConfigMap remains at 0. Each patch clears the old explicit ID cohort,
and sets the fleet percentage. Independent segment-adoption and capacity-recovery
switches, runtime observation, durable gates and all reader switches are preserved.
Initial fleet stages require explicit false readback of adoption/recovery before
any mutation; the release-controller owner must integrate that refusal. The
application tests parse every patch and preserve both independent switches in
all four Boolean combinations. See the [owner activation packet](CMA_Agent_Log_Activation.md)
for complete-operation limits, individual approvals and title/approval HOLDs.

After the release owner approves the image and stage, the controller can patch
`chrono-bot/cma-agent-manager` with the selected file (`kubectl patch configmap
cma-agent-manager -n chrono-bot --type merge --patch-file <stage-file>`), then
perform its reviewed same-image API rollout. These files are patch payloads, not
standalone resources for `kubectl apply`. Check explicit Deployment environment
overrides before rollout and verify effective values on every serving process.
Applying a ConfigMap patch alone does not change an existing process.

Use `off.json` for the fleet kill switch; it remains usable while independently
approved adoption/recovery support is enabled. Disabling those independent
controls is a separate operation: close the relevant durable admission gate,
reconcile/drain accepted uncertainty, then disable process dispatch according to
[segmentation operations](CMA_Log_Segments_Operations.md). Already committed
recovery work, exact receipt reconciliation, compatible reads and cleanup remain
available. Keep the
qualified reader configuration and original history; an off patch does not lower
the compatible-image floor or prove that an uncertain operation aborted.

## Lazy preparation and failure isolation

Before a selected ordinary message reservation, conversation commit or
single-event append opens its write transaction, a scheduling helper may prepare
that Agent. It first uses the common read-only proof in a snapshot. Matching
committed metadata is reused, including after restart. Otherwise it calls the
unchanged per-Agent preparation transaction: complete validation and semantic
audit, revision/head/incarnation fencing and confirmed metadata-only commit.
Other append paths retain their existing complete fallback, which also maintains
trusted metadata as part of the authoritative append transaction.

Proactive preparation has one slot per process, a 10-second deadline and no
queue or detached tasks. Simultaneous requests for the same Agent are suppressed;
other requests finding the slot busy use the existing full path immediately.
This bounds additional preparation work; it does not impose a new fleet-wide
limit on existing fallback scans. Multiple API processes each have one slot,
while Mongo transaction/CAS rules retain authority across processes.

A scheduling cache holds at most 1,024 Agent deadlines. Successful preparation
or proof reuse suppresses redundant preflight for 30 minutes from completion.
Every attempt reserves a 60-second cooldown from its start, including on failure
or cancellation; a ten-second timeout leaves about 50 seconds of that cooldown.
Eviction only permits another preflight.
The cache is never evidence: every actual read/write validates its own fresh
snapshot through the existing proof. A stale checkpoint from an old writer
therefore falls back and is refreshed by the existing full append path even
while a scheduling cache entry is present.

Timeout, unavailable metadata, conflict or unconfirmed preparation commit does
not disable the process or another Agent. The ordinary write path decides its
result. A quarantined or corrupt Agent remains fail-closed; failure isolation
never means bypassing a quarantine marker. Cancellation drops the permit/session;
no work continues in a detached preparation task. Public reads never prepare,
repair or quarantine: an unprepared selected Agent retains complete read-only
validation until a real write supplies valid metadata.

## Aggregate observations

The existing ten-second telemetry emission includes
`cma_agent_log_activation_total` with these bounded `outcome` values:

| Outcome | Meaning |
| --- | --- |
| `prepared` | A proactive full preparation completed and its commit was confirmed. |
| `reused` | A proactive check reused valid committed proof without a full audit. |
| `preparation_failed` | Proactive work failed or exceeded the deadline; the ordinary path remains authoritative. |
| `preparation_busy` | No proactive slot was available; the request proceeds through its existing path. |
| `fallback` | A selected reader/writer lacked trusted proof and entered its complete path. |
| `quarantined` | A selected proof observed a quarantine marker; an observation count, not a unique-Agent census. |
| `quarantine_written` | An existing audit/writer quarantine update matched its fence, in any activation mode; counts writes, including repeated markers, not unique Agents. |
| `write_scan_avoided` | A trusted append/admission/receipt proof replaced its complete validation scan. |
| `window_scan_avoided` | A successful selected page/range avoided complete validation. Full-prefix reads are excluded. |

These process counters contain no identifiers, user content or error strings.
They reset at process restart; aggregate deltas per process/boot rather than
subtracting across a rollout. Cancelled tasks leave their scheduling cooldown;
existing failed scan/cursor/checkout telemetry retains interrupted work. Counts
of successful preparation are not a distinct prepared-Agent inventory.

Use existing full-scan counts/bytes/duration, consumed-read documents/bytes,
checkout rate/wait/failures and HTTP latency/503 metrics to measure the net effect.
Include first-use preparation cost and unprepared, busy, failed and stale-proof
fallbacks. An avoided validation scan is not a claim that every semantic
consumer or full-prefix read is bounded. #1349 remains separate.

Checkout summaries are per-emission windows: sum their counts, durations and
histogram buckets once, and report checkout quantiles as bucket bounds. Route
latency quantiles use pooled request durations with sample counts. Activation
and scan counters are cumulative: compute boundary deltas within one process
birth, carrying sparse samples forward without treating absent telemetry as zero.

## Gates and production acceptance

Unit contracts cover stable nested buckets, default/invalid/mixed config, more
than 16/171 selected candidates, bounded concurrency/cache, failure isolation,
timeout and cancellation. Real-Mongo contracts cover first append, durable reuse
after restart, exact command replay/expired authority, quarantine isolation,
old-writer protocol races, stale-proof repair, disable/re-enable and public-read
immutability. Existing unknown-commit, dispatch, semantic-audit and public-window
contracts must remain green. Protocol-shaped old-writer tests do not claim an
executed old-binary qualification.

Release owner uses a fully green develop batch artifact. At every stage record
source/image, percentage, eligible natural request counts, first-preparation and
steady-state behavior, checkout wait p50/p95, admission/open-Agent p50/p95, scans/s,
bytes/s and 503s over matched windows. Preserve the release owner's existing
admission activation gates: a minimum 30-minute off baseline, separate rollout
and first-use costs, old-process drain, the initial health watch, then at least
60 steady minutes at each stage before expansion. Require 100 successful natural
ordinary-message samples on each side before claiming its p95 improvement;
report open-Agent samples separately and leave missing coverage unverified.
Keep image, replicas, workers, pool and resources fixed through each comparison.
Retain separate incident handling for known unhealthy/full-ledger Agents;
selection does not certify their recovery or eligibility. No production test
messages or database edits are authorized to the implementation owner.

Stop expansion and use same-image percentage-zero rollback on correctness or
receipt drift, read-side mutation, new quarantine/corruption signals, sustained
preparation/fallback storms, checkout-wait regression, HTTP latency regression
or new 503/no-healthy-upstream events. Preserve all durable evidence and diagnose
the affected Agent independently. The owner's updated complete-operation deadline
is 14 November 2026, 14:00 SGT, as specified in the activation packet; neither a
favorable p95 nor an early ACK passes a hard completion limit.

## Local first-use and steady-state measurements

Disposable Mongo 8.0.13 CMAP counts include the full operation, with approximately
4 KiB retained message records. The high-history append fixture leaves room
below the unchanged 10,000-entry normal limit. First append performs one full
preparation scan; the next append performs no full scan at either size.

| Retained rows | Unprepared 200-row page checkouts | First append checkouts | Steady append checkouts | Prepared 200-row page checkouts |
| --- | ---: | ---: | ---: | ---: |
| 1,000 | 3 | 13 | 6 | 4 |
| 9,990 | 5 | 15 | 6 | 4 |

The prepared page returns identical rows and uses the shared proof. It consumes
201 log documents including the proof tail, versus full history on fallback;
the existing public-window profiler verifies 202 examined documents across all
collections at 1,000 and 10,000 retained rows. Small unprepared histories can
have fewer checkouts than the bounded proof. The first write also pays the lazy
preparation cost. These observations separate decoded-history savings from
checkout savings and do not predict fleet latency or replace matched production
qualification. Busy, stale, failed and semantically ineligible fallbacks remain
part of the production denominator.
