# Agent provenance backfill progress

This first #1331 slice records the existing Agent provenance worker's bounded
observations and sweep receipts. It supports the migration standard (#1338),
service-owned execution work (#1328) and M-OPS-02 (#1554). Runtime routing and
endpoint cohorts are not covered.

## Activation and ownership

Ordinary startup does not create or initialize progress storage. Without the
`agent_provenance_progress` collection the existing paced worker runs as before.
An explicit operator action provisions the collection/control row:

```
cma --print-provenance-progress-role
cma --provenance-backfill-status
cma --provenance-backfill-init
```

Use the release image and the existing explicit `CHRONO_MONGODB_DATABASE`,
`CHRONO_MONGODB_URI` and `CHRONO_MONGODB_AUTHORITY_MODE` settings. CLI deadlines and
sanitized errors keep credential contents out of diagnostics. The command checks
Agent Manager authority; it does not create users, change grants, load master
keys, serve traffic or run the entire application composition. Initialization
requires a supported replica set/mongos and refuses nonempty unknown history,
unsupported control schemas and unexpected collection/index options. Repeating
initialization preserves existing progress and receipts.

The optional role adds only find/insert/update/createCollection/listIndexes on
Agent Manager's progress collection plus listCollections on its database. An
isolated runtime may retain its original role before activation; if progress
grants are supplied they must be complete and confined to this collection. The
printed role is a declaration, not proof of a live grant. The release operator
must provision authorized grants before activation. Shared-schema mode checks
existing effective privileges. No production grant change is implied by merging.

An older isolated binary can reject the additional grants before serving traffic,
because its exact collection allowlist predates this role. Qualify the candidate
and compatible forward recovery artifacts with the intended grants before role
adoption. In shared-schema mode, older binaries may keep the legacy worker;
counts still describe only the durable executor's own observations. Neither
case permits claiming global completion or deleting the progress history.

After activation, missing control, unsupported state, missing completion receipt
or unavailable authority stops this maintenance path. It never falls back after
a progress error. An already activated worker also refuses if the collection
disappears. Do not drop this collection: a restarted older or unactivated process
cannot infer the history of a deleted activation marker. Ordinary API readiness
is independent of maintenance completion.

## Durable observations

The control row retains protocol/schema versions, sweep identity, stable Agent
ID cursor, server-time lease, fence, revision, counters and timestamps. Each step
processes at most one string Agent ID in ascending order. A transaction claims
or renews the 30-second lease, applies the existing provenance/lineage checks,
then commits the effect and cursor/counters together. A concurrent executor must
serialize on the control row and a stale lease cannot settle an observation.
There are no external effects in this transaction. Submitted DDL is not protected
by these transactional fences; DDL belongs to the separate migration runner.

An ambiguous commit returns unavailable. No speculative counter or new attempt
is published. The next call reloads durable state: either the original effect and
observation committed together, or neither did. It cannot count that insertion
again as a retained observation. A timeout/shutdown has the same unknown-outcome
rule. Existing provenance, domain identities, history and runtime attachments
retain their original semantics.

Missing or corrupt ancestry aborts the effect transaction. A separate observation
transaction advances only the same unchanged cursor/revision and retains an
internal exception row with the run, Agent identity and sanitized reason. Driver,
conflict and unknown errors do not advance progress. Exception history is never
rewritten into later success; subsequent sweeps may observe repaired records.

Counters obey `scanned = recorded + retained + skipped + errors` within a sweep.
`recorded` means this sweep inserted provenance; `retained` means a verified
existing record; `skipped` means missing evidence; `errors` means corrupt/capacity
refusal. Transient failures remain unresolved at the cursor, rather than being
reported as a completed error observation. Non-string Agent IDs are outside this
existing worker's scan contract and require separate corruption inventory.

The worker keeps the shared background budget, 30-second startup deferral,
15-second operation deadline, 250 ms pacing and five-minute interval between
sweeps. Lease contention defers work. Legacy page scans remain bounded at 25;
activated progress uses one-item transactional scans. Activation can overlap a
legacy worker: immutable provenance and lineage fences retain safety, but counts
represent only this durable sweep's observations.

## Status and completion evidence

An empty next scan atomically inserts an immutable `sweep_finished` receipt and
settles the control pointer. Receipts and exceptions have no TTL or deletion path.
The status command performs bounded reads of the control and its last completion
receipt and returns aggregate counts, phase, sweep identity, revision/fence and
timestamps. It omits cursor/Agent identities, worker identity and raw driver
errors. Internal exception details remain available only through authorized
service-owned database inspection.

A finished sweep is **not global backfill completion**. Concurrent arrivals may
appear behind its cursor and exceptions may remain. Status always reports
`global_completion: false`; activation alone never proves completion. Full issue
acceptance needs a stable/reconciled cohort, reviewed exceptions, routing and
endpoint receipts, and qualified production evidence.

Release evidence must bind the exact source SHA and image digest, environment,
configuration, time window, cohort counts, raw receipts and preserved original
facts. Counts do not establish artifact provenance. Candidate changes during a
sweep require retaining the participating release identities and requalifying
their relevant behavior. The release owner controls activation and production
verification; this implementation work performs no production reads or writes.
