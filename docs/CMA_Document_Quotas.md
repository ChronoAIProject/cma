# Document count and retained source quotas

Part of #1472; implementation issue #1661. Migration record:
`2026-11-11-am-document-source-quotas`.

`CHRONO_DOCUMENT_QUOTAS_ENABLED=false` is the deployment default. After a complete
storage baseline is installed and the switch is separately enabled, every new
immutable source put reserves capacity in Mongo before object I/O. One transaction
serializes an owner's reservations across API processes. Limits are 100 document
identities, 1 GiB retained source bytes and 32,768 distinct source objects per owner.
The object count bounds the ledger even for tiny failed writes. No in-memory
counter grants capacity, and storage/transaction failure refuses the write.

Document identity and content-addressed object identity are idempotent. Retrying
an already reserved object consumes no additional count or bytes, including at
capacity. Different content under an uncertain create key still consumes retained
bytes. Failed upload replies, cancellation and metadata conflicts keep their
reservations because an immutable object may already exist. Release requires proof that an object is unreferenced and its deletion has
completed; an HTTP failure alone never refunds a reservation. There is no TTL or
reset. Retirement of an Agent does not retire its documents.
Existing over-limit data is retained; fresh growth is refused with the existing
typed `document_capacity` response. Unknown baseline or unavailable accounting
fails closed through the existing storage-unavailable response.

Reads, history, title-only changes, sharing and owner grant/link revocation do not
reserve source storage. Exact committed metadata retries precede quota admission.
Runtime publication revalidates authority before reservation and after upload.
Document deletion and the separate #1480 retained-data deletion acceptance remain
separate work. The gated reclaim worker below handles ledgered orphan sources.

## Durable format and bounds

Agent Manager owns `cma_document_quotas`, added to its exact schema/role registry
and startup version guard even while the feature is off. Rows use schema version
1 with the `_id` and `{kind: 1, state: 1, reserved_at: 1}` indexes: one activation marker, one owner aggregate, one
document reservation per owner/document and one object reservation per
owner/document/digest. Owner identifiers are SHA-256 digests. Object rows retain
only the charged length; no source text, title, credential or grant is copied.
Existing document, revision, receipt, object-key and storage-generation formats
are unchanged. Reclaim removes only condemned object reservations after successful
object deletion and decrements their owner accounting in the same transaction.

Admission uses point lookups and bounded transaction retries under a three-second
deadline. Uncertain commit outcomes remain charged or are safely retried under
the same immutable identity. Default-off source writes perform a bounded marker
read; they never silently bypass an existing activation fence. A prepared marker
refuses source puts for both flag values. A ready marker requires the enabled
flag, so flag-only rollback cannot create untracked objects.

The operator import accepts at most 32 MiB and 100,000 unique source objects.
Referenced metadata is scanned in batches of 16 under a bounded query, capped at
100,000 documents. Source and metadata entries must agree on owner hash, document,
digest, object key and bytes. An individual source is at most 1 MiB. Imports retain
over-limit baselines, including orphan objects, without granting additional room.
Each CLI invocation has a 60-second deadline and checkpoints completed rows every
32 majority writes/readbacks. Retain the exact manifest bytes for safe resumption.

## Prepare, inventory and install

These are manual operator actions with their own production window. This PR does
not execute them or claim a complete storage inventory. Before preparation:

1. Deploy compatible code OFF to every source writer, update the narrow Agent
   Manager collection grant and verify readiness. Retain a qualified compatible
   rollback image. Older writers bypass quota accounting and cannot coexist with
   enforcement; older exact-role validators may reject the expanded role.
2. The release/migration owner records the actual source SHA, image digest,
   complete writer census and recovery artifact. Current-source restart tests do
   not certify any prior executable. Do not activate with an unknown reader floor.
3. Reserve the maintenance slot with the document/storage owner. Prepare blocks
   new source writes but preserves metadata reads/revocation. Coordinate with the
   active acceptance executor and retain the existing permanent fixtures.

Run the same binary with the existing Mongo environment and authority mode:

```text
cma --document-quota-status
cma --document-quota-prepare <new-canonical-UUID>
```

The UUID identifies this one-way initial activation. Preparation is idempotent
only for that identity. Then prove all pre-fence source uploads and metadata
commits have drained. Do not substitute a fixed sleep for writer/drain evidence.
Capture a complete storage-owner inventory of the entire CMA `documents/` prefix,
including every immutable object version, retained orphan and uncertain write,
and reconcile it against all retained document revisions. Stop if storage
versioning, deletion markers, pending puts or another writer make accounting
ambiguous; do not certify only the Mongo-referenced subset.

The private JSON manifest has exactly these fields:

```json
{
  "schema_version": 1,
  "activation_id": "<the-prepared-canonical-UUID>",
  "complete_storage_inventory": true,
  "writers_drained": true,
  "objects": [
    {"owner_hash": "<64-lowercase-hex>", "document_id": "doc_<32-lowercase-hex>",
     "sha256": "<64-lowercase-hex>", "bytes": 123}
  ]
}
```

The two boolean attestations require the named owners' independent receipts. The
importer can verify references and bounds; it cannot prove that an external object
store listing is complete. If the storage service retains multiple physical
versions at one immutable key, this one-object-per-key schema is insufficient:
keep activation blocked until version inventory is reconciled by a reviewed
migration. Do not discard those bytes from the acceptance calculation.

```text
cma --document-quota-install <private-manifest-path> --dry-run
cma --document-quota-install <same-private-manifest-path> --apply
cma --document-quota-status
```

Dry-run validates the manifest and retained references without binding or writing
ledger rows. Apply binds its exact SHA-256, installs rows without overwriting,
rechecks references/counts and publishes ready last. Interrupted import stays
fenced; repeat the same file to resume. Different content or activation identity,
corrupt/conflicting rows and unexplained extra rows refuse. No reset/abort command
exists. A wrong partially installed baseline needs a separately reviewed forward
repair; never delete the marker, adjust counters by guess or enable legacy writers.
CLI output is aggregate phase/progress only; keep manifests and receipts private.

## Activation, recovery and acceptance

Only after ready readback, all-writer convergence and storage/release approval,
enable `CHRONO_DOCUMENT_QUOTAS_ENABLED=true` on every API process. Verify original
document/source reads, exact retries, owner count/byte boundaries, competing
processes and failed/cancelled writes using the approved bounded harness. No new
production document or source edit is authorized by this document; the retained
fixture contract remains binding. Larger inventories or cleanup need their own
explicit scope and window.

Baseline installation is an additive expand/install/enforce migration and does
not delete recovery units. Separately activating reclaim does delete eligible
objects; retain a qualified pre-activation backup as recorded in the reclaim
migration. Retain
the private inventory digest, writer/drain receipts, dry-run result, aggregate
import progress, ready state, deployed source/digest/config and compatible rollback
receipt. Startup collection creation/version checks have no new wall-time bound;
qualify readiness/index/role behavior before the activation window.

Rollback uses the qualified compatible image and retains all ledger rows. Setting
the flag false deliberately blocks new source writes after preparation; it never
refunds uncertain capacity or restores unaccounted legacy writing. Read/revoke and
committed metadata replay remain available. Recovery is forward-only when the
baseline is incomplete or uncertain. Storage-owner authorized orphan cleanup and
reservation release must prove object finality and preserve exact
operation replay in their own migration; neither is inferred from HTTP failure.

## Source object commit states and bounded writers

Each object ledger row now carries `state` and `reserved_at`. A newly reserved
row is `reserved`; the metadata commit transaction changes it to `committed`,
and the later reclaim workflow may move an unreferenced row to `condemned` before
it is deleted. The writer and reclaim paths update the same object row, so a
condemnation cannot race a metadata commit into a referenced deleted object.
Rows written by older readers without `state` are read as `reserved` until a
compatible writer observes and commits them.

Reservation retries compare object identity without charging a second time, but
they refuse an identity-matching row whose state is `condemned`. This refusal
happens before the source upload and leaves the owner counters and condemned row
unchanged, preventing a late retry from recreating an object after reclaim has
removed it.

When the quota ledger is ready, a document create or Markdown update writes its
metadata and marks its source object committed in one majority MongoDB
transaction. A failed CAS aborts the transaction and leaves the object reserved;
a condemned object makes the transaction unavailable and leaves metadata
unchanged. Unknown commit outcomes are resolved by a snapshot read at the
recorded operation time and succeed only when both the expected metadata and a
committed object are visible. Title-only, sharing, revocation and comment
mutations retain the plain metadata CAS path.

The metadata commit transaction has one three-second overall deadline covering
all attempts and reconciliation reads. The deadline can expire after
`commitTransaction` has been sent, so the caller receives `Unavailable` even
when the final commit result is uncertain. Create callers retry the same
idempotency key through `creation_replay`; update callers retry through the
stored receipt, allowing the next request to reconcile the durable result.

Every API process admits at most 16 source writes at once. The permit covers
reservation, object upload and metadata commit. A seventeenth source write is
refused immediately with HTTP 429 `document_write_busy` and `Retry-After: 1`;
it is never queued. Metadata-only mutations do not consume these permits.

## Data migration record

- **Migration ID:** `2026-11-13-am-document-source-commit-state`
- **Stores and writers:** `cma_document_quotas` object rows and
  `cma_document_artifacts` metadata rows; the Layer 2 document service, Mongo
  document store, quota baseline importer and later reclaim worker are the
  writers/readers.
- **Format:** object rows add `state` (`reserved`, `committed`, or
  `condemned`) and `reserved_at`. Metadata rows are unchanged.
- **Reader floor and gates:** the identity-only quota reader must be deployed
  and become the rollback target before any writer emits `state` or
  `reserved_at`. Existing object rows without `state` are treated as
  `reserved`; the first reclaim/commit observation may advance a referenced row
  to `committed`. The quota and source-write switches remain default-off until
  the ready activation is verified.
- **Migration bounds:** this is additive and does not rewrite existing rows or
  delete objects. It changes only source-bearing create and Markdown update
  commits; metadata-only mutations keep their existing commands.
- **Recovery and backup:** retain the Mongo snapshot/backup and the compatible
  reader image. If a commit result is unknown, reconcile from the transaction's
  snapshot time; if the object is not committed with the expected metadata,
  return unavailable and leave the reservation for reclaim.
- **Reservation safety:** exact identity retries refuse `condemned` object rows
  before upload and leave their counters unchanged, while `reserved`,
  `committed`, and legacy rows without `state` retain the free retry behavior.
- **Commit deadline:** the whole metadata commit and reconciliation path has a
  three-second deadline. A timeout may follow an already-sent commit; return
  `Unavailable` and rely on the same create idempotency key or update receipt to
  reconcile the outcome.
- **Completion:** verify old rows, exact retries, condemned write rejection,
  CAS conflict behavior and the 16-slot busy response before enabling any
  production writer.


## Gated reclaim and finality

Automatic reclaim requires all three prerequisites before scanning candidates or
writing any row:

1. `CHRONO_DOCUMENT_RECLAIM_ENABLED=true` (strict boolean, default `false`).
2. `CHRONO_DOCUMENT_QUOTAS_ENABLED=true` and a valid ready activation marker.
3. The separately scoped maintenance client proves deletion support. Its
   `CHRONO_DOCUMENT_OBJECT_DELETION_ENABLED` switch stays `false` until the E1
   storage contract and E2 scopes in [Dependency contracts](Dependency_Contracts.md)
   are independently verified.

Before activation, deploy this release's reservation-ID writer to every reserving
process and drain older reservers. The PR-3 commit marker remains the minimum
metadata publication contract; same-key incarnation fencing additionally requires
this release's reservation writer.

A missing prerequisite records a blocked reason without candidate scans,
condemnation, deletion or accounting changes. In particular, unsupported deletion
must never leave a reservation condemned and permanently refuse a same-key save.
The worker shares the process background budget, waits 30 seconds after startup,
and runs a bounded 30-second step every five minutes. Budget denial skips that
round without queuing database work. It processes at most 64 rows per round and
at most four object deletes concurrently. A deletion failure stops new deletes
in that batch, retains condemned rows, and respects a longer `Retry-After`.

The object state machine is `reserved -> committed` in the metadata transaction,
or `reserved -> condemned -> deleted` through reclaim. Legacy rows without a
state are reserved. New reservations include a unique `reservation_id`; judgement,
finalization and unknown-outcome reads match the captured reservation incarnation.
A stale candidate cannot release a fresh reservation under the same content key.
Legacy rows without an ID match only another missing ID, so older reservers must
be drained before reclaim begins. A candidate must be older than the 30-minute grace period;
its judgement transaction rereads both the document and its age. A revision
referencing the exact key repairs the row to committed. Otherwise the conditional
condemnation writes the same row as the source commit transaction, forcing the
writer or the judge to lose their write conflict. The grace reduces aborted slow
writers; it supplies no concurrency proof.

Deletion runs outside transactions. Every condemned candidate, including a row
just condemned in this round, must first acquire an exclusive deletion lease.
The atomic claim matches the reservation incarnation, sets the worker's unique
startup `delete_holder`, increments `delete_fence`, and sets `delete_lease_until`
for five minutes. An existing lease can be taken over only after strict expiry;
even its current holder cannot issue another delete under the same active lease.
Without a successful claim there is no delete or finalization. Immediately before
delete, remaining lease time must exceed the entire 30-second storage operation
budget plus a five-second margin; elapsed claim and scheduling time count against
this allowance. A short lease is abandoned without sending a request. Failed or
cancelled work retains its lease until expiry, then another round may claim it.

Both deleted and already absent responses permit finalization. A separate
transaction increments the owner fence, deletes only a condemned object row
matching the reservation incarnation, deletion holder and deletion fence, and
decrements bytes and objects only when that row
was removed. This serializes with reserve and makes accounting release exactly
once across processes. It also releases a failed-create document slot only after
the main document and its last object row are absent. Negative counters abort
with corruption and preserve the transaction's rows. Unknown commits are
reconciled using the transaction operation time; ambiguous work remains retryable.

New baseline installs retain immutable `baseline_documents`, `baseline_bytes`
and `baseline_objects` on owner rows, and publish a server `activated_at` timestamp
on the activation marker. Legacy objects with no reservation time can use that
activation timestamp. If both timestamps are missing, reclaim skips them rather
than inventing their age. Operators must investigate those rows with a private
inventory before certifying a complete sweep.

Release means finality was proved, followed by accounting settlement. Under a
healthy dependency and a batch without backlog, grace plus interval plus step
is about 36 minutes. Larger queues, denied budget or dependency backoff can take
longer; the metric age is the operational evidence, not a fixed completion SLA.
Referenced revisions remain retained. Removing an Agent still does not remove
its independent documents.

## Reconciliation and authorized orphan cleanup

The authorization policy distinguishes two populations. Ledgered, transactionally
proved unreferenced reservations are eligible for automatic reclaim under the
approved policy. Objects outside the ledger require an operator's explicit
report review, dry-run and apply. Neither command authorizes document deletion,
checkpoint cleanup or changes to another service's data.

Run with the service's existing Mongo authority and storage environment:

```text
cma --document-quota-reconcile --report /private/document-reconcile.json
cma --document-quota-reconcile --report /private/document-reconcile.json --resume
cma --document-quota-reconcile --report /private/owner-reconcile.json --owner <owner-hash>
cma --document-quota-orphan-cleanup /private/document-reconcile.json --dry-run
cma --document-quota-orphan-cleanup /private/document-reconcile.json --apply
```

Reconciliation is read-only against the database and storage. It pages for at
most 60 seconds per invocation, preserving progress in the private report for
`--resume`. Keep its path and contents private: keys can appear in the report,
but never owner subjects, titles or document bodies. Standard output is aggregate
JSON only. Every invariant is `PASS`, `FAIL` or `NOT_CHECKED`; partial scans and
unavailable capabilities must not masquerade as successful zero counts. The
report records the age observation interval. When resuming after ledger progress,
I3 is `NOT_CHECKED` because earlier row ages were not reread; start a fresh report
for current age evidence. Reports are bounded to 32 MiB, with a smaller staged
state limit reserving serialization space; if capacity is exhausted, start a
new owner-scoped report. Resume and cleanup bind to a digest of the database URI,
database name and storage bucket; configuration or credential changes require
fresh reconciliation. Repeat the same `--owner` when resuming an owner-scoped
report. If listing becomes available, resume can continue its
previously unchecked listing phase. Duplicate keys or cycling cursors refuse the
listing evidence rather than double-counting objects. Reads are bounded majority
pages, not a single cross-invocation database snapshot; review concurrent changes
and repeat a failed check in the approved observation window.

- I1 compares owner counters with their document and object rows.
- I2 requires a committed object row for every revision reference.
- I3 checks reserved age against grace plus interval and condemned age against
  one interval. Purge lifecycle age is outside this release's scope.
- I4 checks listed keys against ledger rows, counts outside keys as
  `orphan_blob_count`, and excludes referenced keys from cleanup. Without listing
  capability it is `NOT_CHECKED`, with an unknown count rather than zero.
- I5 computes `count_overshoot=max(0, documents-100)` and
  `bytes_overshoot=max(0, bytes-1073741824)`. Existing baseline overshoot is shown
  separately; owners with retained baseline counters can be checked for growth.
  This compares current usage with the baseline; proving no historical growth
  requires retained observations throughout the activation window.
  Legacy baseline comparison is `NOT_CHECKED` when the original counters are
  unavailable; current usage cannot reconstruct the baseline.

The maintenance interface currently lists only a single owner or document
prefix. Global reconciliation enumerates owners known through ledger or revision
records and explicitly reports this limited coverage. Its I4 is `NOT_CHECKED`
and `orphan_blob_count` is `null`; `observed_orphan_blob_count` contains the extras
actually found under known owners. Only an explicit `--owner` scan can return
a definitive logical-key orphan count or I4 `PASS` for that prefix. It cannot prove absence of
objects under an unknown owner prefix. Obtain the independent complete inventory
from the storage owner for global acceptance. Listing covers logical keys only;
zero logical orphans does not prove that hidden versions or soft-deleted bytes
are gone. Retain the provider's physical-version/finality evidence separately.

Before applying an outside-ledger cleanup, stop source-writing admission and
prove all in-flight source uploads and commits have drained. Keep the ledger
ready. This operational exclusion is required because absence rechecks cannot
fence a new reservation for a previously absent content-addressed key. Do not
use a fixed sleep as drain proof. Review the completed report and run dry-run,
then apply the same file. Apply writes a private `<report>.audit` JSONL intent
with the report digest and aggregate count before any delete, followed by its
aggregate outcome. An intent without a final result means the effect is uncertain.
Both modes refuse an unready ledger; apply rechecks
every key for ledger absence and absence from every revision before deletion.
Dry-run changes no database or object. Retain aggregate receipts and the report
digest for audit. An error or uncertain delete requires fresh reconciliation;
never infer absence from a failed request.

## Reclaim observations and rollback

Background rounds emit `cma_document_reclaim_pending_objects` (stale reserved
plus condemned), `cma_document_orphan_oldest_age_seconds` and
`cma_document_reclaim_blocked`. Counters use
`cma_document_reclaim_objects_total{outcome}` with `committed`, `condemned`,
`released` and `delete_failed`. Counts are observations of this process's work,
not a second durable ledger. No online request gains a metrics query. Blocked
rounds leave backlog observations unknown or at their last successful sample;
consult blocked status before interpreting them.

Alert when orphan age exceeds 86,400 seconds or blocked status persists after
activation. Check E1/E2, activation, all-writer versions, budget pressure and
retry delay before intervening. A delete failure must retain its reservation.
The deletion lease serializes competing reclaim workers during its validity;
all reclaim processes must use this lease-aware release before activation.
The remaining storage-owner acceptance (E1 item 5) must prove an abandoned or
timed-out DELETE cannot take effect after the lease expires against a new
generation under the same key. Local cancellation and holder/fence checks cannot
cancel remote effects. Keep deletion/reclaim closed until the storage owner
demonstrates that finality or equivalent generation fencing contract. Drain any
older reclaim workers before enabling this release.

Disable `CHRONO_DOCUMENT_RECLAIM_ENABLED` and restart to stop new rounds. Retain
all additive rows, indexes and compatible source writers. Reclaim activation
requires this release's reservation-ID writers; rollback with reclaim off retains
the PR-3-or-later commit-marker floor. Closing
the switch does not restore an already deleted object. Restore only through an
independently verified backup/recovery procedure that preserves reference and
ledger consistency; this release grants no restore or production cleanup action.
