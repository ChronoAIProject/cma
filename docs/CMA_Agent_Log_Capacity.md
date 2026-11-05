# Agent log capacity: incident, settlement and continuation choices

Design for [#1321](https://github.com/ChronoAIProject/cma/issues/1321).
The [production investigation](https://github.com/ChronoAIProject/cma/issues/1321#issuecomment-6635920585)
records two affected Agents in two Sandboxes, each at exactly 10,000 entries.
The cleanup Agent has failed `CloseProcess` since October 19; the reconnect
Agent repeatedly fails `PublishConnection` at approximately 18-minute intervals.
No production changes are authorized by this document.

## Observed production scope and cause

Read-only VictoriaLogs investigation searched October 1 through November 4,
16:15 UTC. That is the search window, not a claim of complete archive retention.
Diagnostic downloads matched server-side aggregate counts. Each capacity error
paired with a unique failed work attempt on the same pod within 20 ms.

| Lifecycle step | Capacity failures | Agents / Sandboxes | Earliest retained error (UTC) | Public state at cutoff |
|---|---:|---:|---|---|
| `CloseProcess` | 1,271 | 1 / 1 | Oct 19 18:46:30.248 | `paused`, log head 10,000 |
| `PublishConnection` | 5 | 1 / 1 | Nov 4 14:50:52.647 | `ready`, log head 10,000 |

Of 1,276 observed failures, 1,159 explicitly carry the northeast1 production
cluster label. The 117 earlier AM records lack that label and identify the same
cleanup Agent. The two Sandbox bindings were established from complete runtime
inspection diagnostic sets; no resource IDs or user content are retained here.
Reconnect gaps were 1,078.081–1,080.753 seconds; the cleanup median was 1,080
seconds. Both Agents' last public update matched their 10,000th log entry.

`store/agent_log_integrity.rs` bounds complete, gap-free history validation.
The limits are application safety budgets, already present in September, and
are not a Mongo collection/document-count restriction. Bytes include complete
BSON envelopes and records. No arithmetic or off-by-one accounting defect was
found. The defects are stranded leases after deterministic rejection and
reserves that excluded the preceding cleanup steps needed to reach a terminal
publication. A failed close publication cannot establish whether the native
close already happened; retain its original identity and reconcile its receipt.

#1316 addresses repeated full-history validation during range reads. It does
not replenish append capacity. #1311 can grow logs faster: it retains output
bytes on tool updates and appends output-only events, bounded to 16 KiB per
update. The October 19 incident predates that deployment. Redacted public event
kinds prevent quantifying #1311's contribution to either affected Agent; no
such attribution is inferred from the roughly 30-second tail cadence.

## Owner decision: can an exhausted Agent ever continue?

**Under the current design, reaching 10,000 entries permanently exhausts an
Agent's normal append budget.** There is no time window, expiry, lease reset,
log segment rollover, or automatic reclamation. The alternative lifetime limit
is 64 MiB of complete BSON log documents. A byte-budget rejection can leave room
for a smaller entry, but repeating the rejected append with unchanged history
cannot succeed. A full entry count leaves no normal append room at all.

Every retained event counts, including lifecycle retries, tool output and turn
boundaries. The native provider's context compaction does not free this budget.
Restarting CMA, waiting for the 18-minute lease, refreshing the browser, or
retrying a message does not reclaim capacity. A connected native process may
still exist; CMA cannot promise durable conversation progress once its required
writes are rejected.

The immediate fix settles a deterministic lifecycle failure with a named
`agent_log_capacity_exceeded` condition and releases its claim promptly. For
normal startup/reconnect work, the outcome is `Failed`, retaining the
Agent ID, native conversation, pending recovery facts, home and complete log.
It neither creates a replacement Agent nor claims the native process stopped.
Already-authorized cleanup remains able to complete with its original effect
identities and receipts.

**This settlement does not enable continued work on the exhausted Agent.**
The existing API has no reopen operation for a Failed Agent. Raising limits
alone would not automatically reopen one already settled as Failed. Continuing
the same Agent would require an explicit, tested recovery operation as well as
additional append capacity; ambiguous prior input must never be replayed as
new input. A new Agent is a separate owner decision, with a distinct identity
and history; it is not a transparent continuation of the old Agent.

Existing command reconciliation rejects still-queued input as
`agent_unavailable`. A response already bound to native dispatch becomes
`Failed` / `runtime_lost` under the existing terminal-Agent rule; its original
receipt, dispatch binding and observed delivery remain intact, and the command
is not sent again. Here `runtime_lost` is the existing response classification
for a terminal Agent, not proof that its native process was physically removed
or that its input was never executed. The Agent's named capacity failure gives
the specific cause. Recovery must inspect the retained delivery facts and must
not infer that resending the same text is safe from that response code.

The owner chooses the long-term capacity policy. This PR does not implement any
of the following options without an explicit go:

| Option | Can the same Agent continue? | Benefits | Risks and prerequisites |
|---|---|---|---|
| Raise entry/byte limits | Potentially, with an authorized recovery path if already Failed | Smallest storage-policy change; retains all records and existing sequence numbers | Only postpones exhaustion; increases full-scan CPU, memory, Mongo transaction work and latency. Existing readers reject histories beyond their hard ceilings, so reader deployment and a compatible rollback floor must precede larger writes. Preserve separate stop/delete headroom. |
| Roll over to a new log segment | Yes, after a compatible reader/writer migration and recovery path | Bounds active-segment work while preserving unlimited logical history | Requires durable segment manifests, atomic rollover/fencing, stable global sequence/cursor semantics, cross-segment integrity and replay, and unknown-commit recovery. Old readers that assume one bounded complete log cannot consume it. All retained segments must remain accessible. |
| Compact projections while retaining original events | Potentially; only if accounting and readers explicitly support archived segments | Speeds current-state reconstruction without destroying audit/recovery data | A summary is not equivalent to exact messages, tool records, receipts or original input identities. Keeping originals in the counted log does not free capacity. A checkpoint must prove its covered prefix, retain references and support replay/rebuild across the full archive. Native context compaction alone is unrelated. |
| Archive older immutable segments to durable storage | Yes, with a unified reader and supported recovery path | Reduces hot-store size and can bound normal append accounting | Copying events alone frees nothing. Moving them requires a committed archive manifest, integrity checks, access policy, encryption/key retention, availability and backup/restore guarantees, stable cursors and exact recovery lookup. Archive outages must be distinguishable from missing history. No deletion of retained records to satisfy old readers. |

Recommended direction for sustained long-running Agents is immutable segmented
history with compact current-state projections and archival behind the same
read/recovery contract. That is a storage migration, not an emergency increase
of an unexplained constant. The owner may instead choose a measured limit
increase as an interim policy, with an explicit maximum and reader floor.

## Immediate fix and invariants

Normal append capacity stays at 10,000 entries / 64 MiB. Stop/failure capacity
ends at 10,064 / 64.5 MiB, and delete capacity at 10,128 / 65 MiB. The 16-MiB
per-document and 64-entry per-batch checks remain independent.

1. Treat `AgentLogCapacityExceeded` as a known-aborted deterministic publication,
   separate from Mongo unavailability and unknown commit outcomes. Unknown
   commits retain the existing reconciliation and lease rules.
2. Refresh only the same still-owned, unexpired claim before settling. Preserve
   concurrent committed metadata; reject replaced work, runtime identity,
   changed authority and expired leases. Settlement must be validated and
   committed atomically with its diagnostic event.
3. For normal startup/reconnect failure, publish the named failure promptly and
   remove pending lifecycle work. Preserve runtime identity, receipts and
   cleanup debt. Settle only input proved never dispatched; preserve accepted
   receipt identity and all ambiguous/dispatched input evidence.
4. Allow finite cleanup completions to use the relevant reserve: stop fencing
   and process-close completion use stop capacity; deletion close/home/worktree
   completions use delete capacity. The full domain validator must still prove
   each transition and effect receipt before reserve selection. A completed
   close is never fabricated to work around capacity.
5. Repeated cleanup failures must not consume terminal reserves indefinitely.
   Record one fixed capacity marker per cleanup step, then update only retry
   timing and attempt counts while that marker remains the history head. Both
   stores verify the exact retained marker and proposed retry in the same
   snapshot; an absent marker or duplicate marker is a conflict. Successful
   cleanup still appends its authoritative completion event. Prove transient
   failure followed by success beyond the normal budget.
6. Keep every retained entry and reader, including #1311 tool output. No new
   log segmentation, physical history removal or implicit replacement Agent.
   Surface an actionable remediation instead of presenting repeated retries as
   recovery. Describe any changed persisted semantic shape and reader floor.

The existing finite reserve is a safety mechanism, not a second conversation
budget. Reconnect success events and ordinary retry events must not borrow it.
If physical capacity is exhausted or history is corrupt, the reserved failure
publication can itself be rejected. The worker must retain the original claim
and recovery facts for operator repair; it cannot promise prompt settlement
without room for a valid durable publication. Dimensioned diagnostics distinguish
physical exhaustion from normal append rejection. Never overwrite old evidence
or invent a successful close or settlement.

The capacity diagnostic reuses `WorkStepFailed` and the existing string-valued
failure class; retry metadata uses the existing work record. There is no new
serialized field, event variant, log segment or sequence reset. Metadata-only
publication already exists for lifecycle intent and parent waiting. Unknown
commit reconciliation must still verify the exact aggregate and full retained
history, including when the new log batch is empty. Older writers can read this
retry state but would reintroduce the original capacity loop. Never-dispatched
first input uses #1318's `Failed` / `agent_startup_failed` contract; attempted or
ambiguous first input remains byte-for-byte unchanged. The reader floor therefore
includes #1318 as well as #1311's retained tool-output readers. A recovery build
must keep these readers and the full physical reserve ceilings. No backfill,
DDL, collection reset or retained-record deletion is required.

## Integration and deployment boundary

#1318 is merged and the lead authorized integration on its validated lifecycle
transition. The capacity handler covers ordinary execution and both prepared
startup paths that acquire a fresh lease inside an attempt. Each path settles
only its own current, unexpired claim. #1316 history-reader files and Sandbox
billing/funding files are unchanged. One PR uses `Refs #1321`; the lead owns
merge, deployment and issue closure. Rollover remains an owner decision.

## Verification

Use real disposable Mongo tests for cumulative count and BSON-byte limits;
the in-memory store does not enforce these lifetime budgets. Test exact normal
and reserve boundaries, unchanged rejection across repeated normal attempts,
finite cleanup completion, retry failure followed by successful cleanup,
history/output readback and reopening. Test expired/replaced claims, concurrent
metadata, unknown commits, preserved undispatched and ambiguous command facts,
and prompt settlement before the lifecycle lease expires.

Run affected crates with `CARGO_BUILD_JOBS=4` while iterating. After integration,
run full required local gates once, including source size/structure, before
opening the PR. Do not push further commits to an open PR unless the lead asks.

## Production verification

1. Record deployed source/image and the initial Agent counts, statuses and log
   heads using owner API reads. Preserve the investigation cutoff above; compare
   new VictoriaLogs windows by lifecycle step and distinct Agent/Sandbox counts.
2. Observe for at least 36 minutes (two former 18-minute leases). An old claim
   can survive the rollout until its existing expiry; the fix does not steal it.
   On the next owned attempt, a normal capacity failure should emit
   `Agent log capacity failure settled`, have the named public failure, and have
   no pending lifecycle work/lease. No repeated 18-minute reconnect cycle is
   expected after that settlement.
3. For the already-requested cleanup, require an authoritative close receipt and
   `Stopped`, or a released claim with the original cleanup pending and normal
   retry timing. A transient runtime failure may keep cleanup pending, but its
   fixed capacity diagnostic must not grow the log on every retry. Never infer a
   close from a capacity error or a failed acknowledgement alone.
4. Check `Agent log capacity rejected` by dimension and ceiling; investigate
   physical-reserve/document failures separately. This finite reserve cannot
   repair already-corrupt or physically over-budget history.
5. Read retained history/tail (including durable tool output) through owner APIs;
   compare identity, log continuity and original input/receipt recovery facts.
   Check terminal remediation and queued/dispatched command outcomes without
   sending a message or replaying an input.

All verification is read-only. No production test turn, stop, cancellation,
replacement, cleanup admission, database write or runtime restart is authorized
by this plan. The lead records the result before closing the issue.
