# Bounded pending-approval recovery

Part of #1349; advances #1320. This compatible reader uses retained legacy events
and the existing trusted metadata. It does not enable segmented writers or
complete long-turn recovery, paginated transcripts or incremental usage.

Approval dispatch asks the conversation store for its exact pending requests.
The complete reader reconstructs them from lifetime history. The existing
`PendingAgentApprovals` fold clears its state at a turn start, completion,
failure, interruption, stop or deletion. A validated suffix containing any of
these boundaries yields the identical outstanding set. Request identities,
original sequences, details, runtime locators and ordering remain unchanged;
resolved requests and earlier requests with reused identifiers stay excluded.
The latest boundary is used, so earlier unresolved requests cannot leak into a
new turn. Missing boundaries never establish an empty set.

`CHRONO_AGENT_APPROVAL_BOUNDED_READS_ENABLED` is a strict boolean, default `false`
in code and the API manifest. It additionally requires selection by the shared
explicit `CHRONO_AGENT_LOG_ADMISSION_AGENTS` cohort or the stable
`CHRONO_AGENT_LOG_FLEET_PERCENT` buckets, plus trusted prepared metadata. Reads
never trigger lazy preparation. An active admission or public-window cohort
cannot silently enable this consumer. The title-reader switch is independent.
The release owner controls all activation.

For a selected Agent with more than 200 entries, reuse the shared read-only
proof and captured snapshot, then request the final 200 consecutive rows through
the Agent/sequence index. The common window reader validates codecs, identity,
sequence continuity and creation lineage. Retained suffix BSON is limited to
4 MiB; the currently decoded document and driver batch can exceed that retained
budget. An oversized suffix falls back without returning partial approvals.
The pure domain fold accepts only a suffix containing a reset boundary.

Switch/cohort off, small histories, missing/stale/unsupported metadata, a long
turn without a boundary, or the byte bound use the complete reader. The same
snapshot and plain complete scanner preserve physical-capacity, sequence and
lineage checks. Proof corruption, an extra/missing tail, quarantine or malformed
requested rows refuse explicitly. Neither bounded nor complete reads audit,
repair, prepare, write quarantine or operate transactions. Explicit writer and
audit paths retain durable quarantine ownership. The subsequent approval
publication still validates and fences current authority and runtime state.

The bounded path inherits the shared trust model: it validates requested rows
and the captured tail, not every immutable middle row on every read. Explicit
audits revoke disproved checkpoints. It must not be enabled where the common
trusted-writer/audit contract has not been qualified.

## Validation and rollout

Local paired tests count all CMAP checkouts and consumed log documents against
the original reader at 1,000 and 10,000 retained entries. They compare exact
pending rows, reused/resolved identifiers and ordering. Unit tests cover every
reset boundary and absence of a boundary. Real-Mongo contracts cover disabled
and untrusted fallbacks, row/byte limits, corrupt proof/requested rows, missing
middle rows in every complete fallback and a concurrent resolution after the
snapshot begins. Command traces prohibit writes/transaction control; refusal
compares the exact Agent document. Existing approval publication and dispatch
security tests remain part of the local gate. Results are recorded before PR.

Paired local Mongo 8.0.13 measurements, including all proof and cursor checkouts:

| Retained events | Original checkouts | Bounded checkouts | Original log documents | Bounded log documents |
| --- | --- | --- | --- | --- |
| 1,000 | 6 | 4 | 1,001 | 201 |
| 10,000 | 8 | 4 | 10,001 | 201 |

These successful suffixes save two/four checkouts (33%/50%) and consume 80%/98%
fewer log documents. At 1,000 events, the long/no-reset and oversized suffix
fallbacks each use six checkouts, matching the original prepared reader;
switch/cohort off uses five, missing/unsupported metadata three, and stale
metadata four. The complete read also removes the original duplicate envelope
query. These fixture results are not a fleet latency or savings forecast.

This adds no stored format, index, projection, backfill or writer floor. Ship
with the independent switch false, after a fully green develop batch artifact.
Qualify admission and public windows first. Compare matched natural approval
operations for success/fallback mix, checkout count/wait, scans/bytes, route
latency and errors. A fallback still costs a window attempt before the complete
scan; local fixture improvements do not establish fleet savings. Missing
natural observations remain unverified. Never send a production approval or
create a conversation solely to generate samples.

Existing scan/read counters are fleet/process aggregates, not an approval-only
denominator. Report that attribution limit; do not infer per-approval savings
or a fallback rate without corresponding request-level coverage.

Rollback by disabling only the approval-reader switch and restarting the API
through the release owner. Keep cohort metadata, events, requests and quarantine
intact. Roll back immediately on a changed pending set, missing runtime locator,
read-side writes, corruption/regression, or worsened checkout/latency budgets.
Target Thu 2026-11-12 evening (merge by 14:00 SGT), with Friday morning as the
last fallback; production verification must finish Fri 2026-11-13 15:00 SGT.
