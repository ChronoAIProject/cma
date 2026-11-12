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

Every retained event counts, including recorded lifecycle failures, tool output and turn
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

### Repeated ordinary failure markers (#1788)

With the strict, default-off `CHRONO_AGENT_RETRY_SINGLE_MARKER_ENABLED` switch,
consecutive failures of the same step and class retain one `WorkStepFailed` row.
Changing diagnostic detail alone does not create a new class. Each repeat still
increments attempts and revision, advances ready time and releases the lease;
it preserves last_seq, work, receipts, stop intent and every retained log row.
The first failure, class/step change and successful completion still append.
Capacity and ledger markers retain priority and their exact-detail checks.

The worker point read is only selection evidence. Both memory and Mongo stores
recheck the exact observed tail and the complete metadata proposal in their own
fenced snapshot. Segmented publication uses the same marker validation and its
existing zero-entry publication receipt. No schema migration or new persisted
field is needed; an older or flag-off worker resumes ordinary failure appends.

The separate bounded-close switch and cohort may bound zero-entry verification,
including at-capacity markers, under the first/hourly full audit and conflict
reuse contract in [Admission append](CMA_Admission_Append.md). Without that gate,
every zero-entry retry still fully scans. The 10,000-entry/64-MiB normal limits,
stop/delete reserves and counting of every actual retained row are unchanged.

### Selecting an already-marked cleanup retry (#1583)

Related to #1421 and #1321. The failed-work retry path may select the existing
metadata-only settlement directly when the claimed Agent is still in an eligible
cleanup status/step, its head is strictly above the 10,000-entry normal budget,
and an exact point-tail observation matches the retained capacity marker for
that Agent and step. A normal failure append adds an entry, so it cannot fit that
head. Skipping that predictably rejected proposal removes one complete traversal
from an eligible, uncontended retry. It does not eliminate validation.

`agent_cleanup_capacity_retry_hint` only avoids an unnecessary point read for
ineligible work. `plan_agent_marked_cleanup_capacity_retry` selects only an empty-log
proposal; it cannot admit a first marker. The original
`plan_agent_cleanup_capacity_retry` keeps its known-aborted-capacity contract for
first markers and the fallback. This distinction deliberately preserves the
#1321 contract while allowing the narrower direct selection.

The point read is never authorization. `commit_agent_work` starts a fresh
transaction, validates the complete captured history and physical limits, checks
its exact tail, reconstructs the only permitted retry, and validates the expected
revision, identity, status/work, holder/fence and lease. A changed observation,
gap, invalid schema or extra tail cannot turn the hint into a publication.
Transaction conflicts use the existing retry budget and revalidate each snapshot.
The opt-in #1788 policy above can reuse identical verified history within a
publication and bound repeated marker verification between complete audits.
Unknown commits keep exact receipt reconciliation and refusal; they never grant
permission for a normal append, another settlement or an external effect replay.

Successful effects retain their ordinary completion path and finite reserves.
First-marker admission, byte-only exhaustion, ledger holds and other ineligible
cases retain their prior paths. Direct retries preserve the same retry-delay
policy, including RemoveWorktree backoff, increment the durable attempt count
once, release the claim and retain status, step, receipts, cleanup debt and log
head. The next due time remains the selected publication time plus that policy's
delay. No capacity, event schema, retained history or recovery eligibility changes.

Real-Mongo regressions count actual scanner `find` commands through worker/store
paths: two traversals for a first marker and the known-abort control, one for an
eligible repeat with bounded verification disabled, and fresh traversals on
transaction retries with that gate disabled. They retain full
validation while exercising changed observations, authority/lease refusals,
corruption and physical limits, committed/uncommitted unknown outcomes, seventy
unchanged failed closes, and successful cleanup continuation. These tests prove
work reduction; CPU-core savings require the lead's later measured deployment.

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

## Native terminal settlement at exhaustion

Conversation output first uses the normal append budget. Only a known-aborted
`AgentLogCapacityExceeded` permits a capacity settlement. The immutable proof
captures the Agent, checkpoint and original refused batch; the committing Mongo
snapshot rechecks that the original batch cannot fit the normal budget. Revision,
head and checkpoint CAS, log integrity and the original runtime generation still
apply. An unknown commit never grants permission to choose another outcome.

If the entire native terminal batch fits the stop/failure reserve, the turn
retains its native completed/cancelled/failed outcome. If it cannot fit, or if an
earlier output was refused, settlement uses `FailedForPrompt` with
`agent_log_capacity_exceeded`. The same fold as live failure first flushes the
saved assistant prefix, then appends the typed failure. Text already committed
to the checkpoint and streamed live therefore remains visible after reload when
that complete failure batch fits. The Response is failed; the partial text is
not presented as a successful answer. Refused tool/model output is not invented.

Only if that failure batch also exceeds the remaining stop reserve does the
writer select fixed failure records with private retention. This is a bounded
sequence of immutable candidates: native terminal, failure with the visible
prefix, then private-prefix failure. Each fallback requires a known-aborted
capacity rejection; an unknown commit instead reconciles the exact attempted
candidate. No alternate candidate may be published while its predecessor's
commit remains uncertain. There is still at most one durable terminal.

The existing atomic batch ceiling is 64 entries; a 100-entry proposal is reduced
to failure before validating its terminal receipt, and the ordinary live fold
emits only a few entries per native update. Tests cover actual count overflow,
a 1 MiB final flush beyond the 512 KiB byte reserve, and exact count/BSON-byte
boundaries for prefix publication. One entry or byte beyond the complete public
failure batch selects private retention without partial writes.

The fixed failure records are independent of assistant text size: `TurnFailed`,
the Agent status transition, applicable first-input failure state, and any small
usage observation from the refused batch. The original cumulative and turn usage
checkpoint is carried into every candidate. `store/budget/mod.rs` uses
`UsageUpdated` plus `conversation.observations.usage.current_turn` before closing
the reservation in this transaction. Tests check consumed units, the settled
reservation and public Response usage exactly once. Existing conservative
liability accounting is unchanged.

When the prefix is too large to publish, its entire original checkpoint is
preserved in `agents.agent_capacity_prefix_v1`. It is not truncated. The
owner-facing failure states: partial output could not be saved in the
conversation history, is retained privately, and can be recovered through
support/operator export. The runbook below retains that export procedure. This
private field is absent when the failure fold successfully publishes the prefix.
The prefix is not lost when the native home is later removed.

Both failure variants keep the previous runtime output high-water mark and do
not ACK refused output. The live recorder closes that attachment after recording
the capacity failure and leaves the refused frame in the native journal. Native
success is ACKed only after its full terminal batch commits. Unknown-commit
proof checks Agent, retained log and active receipt, plus the retained prefix
when the selected candidate requires it. Regression tests exercise committed
and unproved outcomes for both visible-prefix and privately retained failures.

Both outcomes leave the Agent `Failed / agent_log_capacity_exceeded`, with no
pending work or live lifecycle lease. Turn completion means the admitted task
finished; the Failed Agent separately indicates that the retained-history budget
cannot support continued work. Original native identity, receipts, cleanup debt,
prior output and pending-input recovery facts remain. No queued prompt is sent by
settlement. A Failed Agent has no ordinary reopen operation; capacity increase,
replacement or recovery is a separate owner decision.

### Exhausting the stop reserve

When even the fixed failure records cannot fit, the transaction refuses without
ACK or partial publication. The owner still sees Working (or the existing
WaitingApproval state), the active turn and previously recorded output; a command
without a terminal remains `in_progress`. Elapsed time never manufactures a
completion. The original checkpoint remains intact. The capacity rejection and
its dimensions identify why publication cannot progress.

The recovery path is an explicit owner stop followed by delete. Stopping a live
Agent is a prerequisite for deletion. If the stop reserve cannot fit the proposed
validated stop step, the normal worker transaction permits its finite
`FenceInteraction`, `CloseProcess` and `PublishStopped` completions to use the
remaining delete reserve. The existing single-marker cleanup retry contract can
also release its lease there; ordinary retries and conversation settlement cannot
spend this reserve. All domain/lease/receipt checks still precede capacity
selection. For an Agent already Failed by native-terminal settlement, delete can
be requested directly: `EnsureProcessClosed` closes the original process before
`RemoveHome`, `RemoveWorktree` and `PublishDeleted`. Tests verify all three actual
runtime effects exactly once, released leases and retained output on reopening
for count and byte exhaustion, both before and after native settlement.

If the physical 10,128-entry / 65 MiB ceiling is also full, stop/delete may return
a typed capacity refusal. Preserve its original claim, effect identity and
checkpoint for separately authorized operator recovery. Do not remove retained
rows, invent a close receipt or resend an uncertain prompt to recover space.

### Transaction cost and trusted metadata

Capacity settlement uses the same validated `trusted_summary` as ordinary
conversation publication when the existing admission cohort/fleet policy allows
it. Exact raw BSON count/bytes, creation lineage, revision, tail hash, semantic
checkpoint and quarantine checks stay in place. Missing, unsupported, stale or
disabled proof falls back to complete validation; corrupt proof refuses. The
reserve decision depends on exact validated cumulative count/bytes plus the new
encoded batch, so it does not require a second audit of an already trusted
immutable prefix. Capacity-marker lifecycle retries still retain their separate
full-history/tail validation contract.

The dedicated real-Mongo timing test prints recorder wall time and each
transaction's first-command-to-commit/abort interval for exactly 64 MiB of BSON,
with prepared cohort on and off. It asserts zero complete scans when enabled and
two when disabled (the refused normal transaction and the settlement transaction),
and checks each measured transaction against 60 seconds. Raw command monitoring
includes Mongo command/checkout waits within that interval. These local N=1 cells
are mechanism evidence; they do not certify production tail latency.

Production #1320/#1475's approximate 1.2 s per 3.7 MiB scan would extrapolate to
about 21 s per 64 MiB scan if cost were linear. That assumption omits tail
contention, so it is not a guarantee against Atlas's 60 s transaction lifetime.
Prefer the already-qualified trusted cohort for these cases; the operator must
record transaction and checkout tails for the exact candidate. A timeout remains
an unavailable/uncertain result with no false ACK or model resend. Do not raise
the transaction lifetime or capacity ceilings to make this test pass.

### Retained-prefix migration record

Migration ID: `2026-11-11-am-capacity-prefix-v1`; owner: Agent Manager backend;
operator: the release/incident owner. Source/executable hashes are recorded in the
candidate evidence ledger. This is additive expansion plus finite settlement of
already-admitted work, not a backfill or general new-work admission. The D1
visibility correction adds no stored format: it prefers the existing public
failure fold and narrows private retention to prefixes that cannot fit.

The old and current `record` schema, message checkpoint version 1, events,
collections, indexes and TTLs are unchanged. The new envelope field contains the
exact old-format message checkpoint. Ordinary readers decode `record` and ignore
unknown envelope fields; Agent saves, lifecycle and control publications use
`$set` and retain the field. Tests compare the complete retained checkpoint and
reopen the store. Active checkpoint carry cannot overwrite it. Unknown-commit
reconciliation requires it when the compact proof included a prefix.

The writer gate is the finite known-aborted capacity proof plus exact snapshot
CAS; private-prefix writing additionally requires that the visible-prefix
failure batch was proved not to fit. No new ordinary work is admitted and the Agent becomes Failed; there is no
startup scan, new configuration, background writer, DDL, backfill or contraction.
Reader deployment is needed for the operator to understand/export the prefix,
but strict predecessor `record` and checkpoint decoders remain compatible. A
source/protocol check does not qualify an older deployed binary: the release
owner must pin recovery artifacts that preserve the complete envelope, including
this field. Restore the entire Agent document with its logs/receipts; never import
only `record` or clear the prefix to make a rollback work. The native runtime image
and home remain unchanged by settlement.

## Production verification (operator, read only)

No production credentials or execution are available to the implementer. The
incident/release operator owns these checks. Record source SHA, candidate image
digest, environment, UTC start/cutoff, per-case N, flags/cohort, log retention and
raw-result location. The historical rollout anchor is 2026-11-05 approximately
08:34 UTC; partition the observation by subsequent candidate/rollout, including
Train E. Pre-existing cleanup cannot count as recovery by a newer image.

Obtain the two exact Agent IDs from authorized incident diagnostics; the public
investigation discarded raw IDs. For a fixed cutoff, query VictoriaLogs through
the authorized read-only LogsQL endpoint. Replace `<CUTOFF_UTC>` and each Agent
placeholder explicitly. Keep absent cluster labels separate from explicitly
labelled production rows. The filter and aggregation syntax follows the
[LogsQL contract](https://docs.victoriametrics.com/victorialogs/logsql/).

```text
_time:[2026-11-05T08:34:00Z,<CUTOFF_UTC>)
kubernetes.pod_namespace:="chrono-bot" kubernetes.container_name:="api"
(kubernetes.pod_name:"cma-agent-manager-"* OR kubernetes.pod_name:"cma-usage-api-"*)
"agent log capacity exceeded"
| stats by (_time:1h, kubernetes.pod_name) count() as capacity_errors
```

Use the same filter/window for `"Agent work attempt completed" "published=false"`,
`"Agent log capacity failure settled"` and `"Agent log capacity rejected"`.
For each exact Agent, add `"agt_<exact-id>"` to the failed-attempt query and select
only `_time,kubernetes.pod_name`; sort by time to compare successive attempts.
The historical loop was about 1,080 seconds. Observe at least 36 minutes after
settlement, or after any old claim naturally expires. Expected: no recurrent
known-capacity reconnect loop; one explicit settlement or authoritative cleanup
progress. Missing samples or incomplete retention are not a pass. Capacity
rejection without later settlement is a failure requiring disposition.

Authenticated owner GETs (no message/control request) verify:

```text
GET /api/v1/agents/<exact-agent-id>
GET /api/v2/agents/<exact-agent-id>/events/snapshot?limit=1
GET /api/v2/responses/<exact-response-id>
GET /api/v2/commands/<exact-command-id>
```

Normal lifecycle capacity settlement should show Agent `failed` and
`failure.class=agent_log_capacity_exceeded`. A prior authorized cleanup may instead
have reached `stopped`/`deleted`; record its real receipt/disposition, never relabel
it. Native terminal-only settlement may show a completed Response with a Failed
Agent; refused output must show a failed Response with the capacity class. Retain
original command/Response/native IDs and history hashes. Read retained history
pages including tool output and confirm continuous sequences without replaying
input. Verify all terminal projections correspond to the same command.

Public Agent GET deliberately does not expose the lifecycle lease. An operator
with production Mongo read authority must use this exact content-free projection:

```javascript
const ids = ["agt_<first-exact-id>", "agt_<second-exact-id>"];
db.agents.find(
  { _id: { $in: ids } },
  { _id: 1, "record.status": 1, "record.failure.class": 1,
    "record.work.step": 1, "record.work.lease": 1 }
).readConcern("majority").maxTimeMS(15000).toArray();
```

Require N=2 found, the reviewed dispositions, `record.work.step="none"`, and no
live `record.work.lease`. If cleanup is intentionally pending, capture the released
claim and typed due/recovery state separately; it does not satisfy a Failed/None
assertion. Do not modify leases or Agent records during verification.

For a failure naming a retained private prefix, the operator may export only
that Agent's `agent_capacity_prefix_v1` under owner authorization to a private
0600 artifact. Verify its Agent scope via the containing document and its native
conversation/generation/turn, original receipt, byte count and hash. Report only
IDs/counts/hash and artifact location; do not paste the prefix into issue comments.
The ordinary active checkpoint remains a terminal receipt; never resubmit the
prefix as a prompt. This export is a content-bearing recovery step, separate from
the content-free status query above.

Close #1321 only after the reviewer accepts count/byte, terminal, stop/delete,
unknown-commit and lease evidence at the exact candidate, and the operator
provides both historical dispositions with retained readable history and the
post-rollout observation. Merging alone does not pass. Combined-PR and post-merge
develop runs belong to the coordinating reviewer; no separate PR or production
mutation is authorized here. Broader lifetime continuation/segmentation remains a
separate M-STAB-03 decision.
