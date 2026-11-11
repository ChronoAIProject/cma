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

Deploy at 0, qualify the compatible image, then release owner stages 10 -> 50 ->
100. Configuration is loaded from environment, so every stage requires a
same-image API rollout and confirmation that all old processes have drained.
No existing dynamic control channel is introduced or extended. Keep the explicit
cohort empty in fleet mode. The kill switch is percentage 0 plus an empty explicit
cohort, rolled out on the same compatible image. It disables both admission and
public-window selection. Independent title/other reader switches remain separate.

Disabling selection does not disable quarantine writes by explicit audits or
full writer fallbacks. Keep every marker, event and receipt. Never remove a marker
to authorize an old image. An older-image rollback needs its own compatibility
qualification; percentage 0 is not evidence that the old binary is safe.

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
or proof reuse suppresses redundant preflight for 30 minutes; failures and
cancellation suppress it for 60 seconds. Eviction only permits another preflight.
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
bytes/s and 503s over matched windows. A minimum 30-minute watch precedes each
expansion; missing natural samples remain unverified. Retain separate incident handling for known unhealthy/full-ledger Agents;
selection does not certify their recovery or eligibility. No production test messages or database edits are
authorized to the implementation owner.

Stop expansion and use same-image percentage-zero rollback on correctness or
receipt drift, read-side mutation, new quarantine/corruption signals, sustained
preparation/fallback storms, checkout-wait regression, HTTP latency regression
or new 503/no-healthy-upstream events. Preserve all durable evidence and diagnose
the affected Agent independently. Target Thu 2026-11-12 morning train (merge by
06:00 SGT), Thu evening fallback; final acceptance Fri 2026-11-13 15:00 SGT.
