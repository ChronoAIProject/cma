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
reservations because an immutable object may already exist. There is no automatic
refund, TTL, delete or reset. Retirement of an Agent does not retire its documents.
Existing over-limit data is retained; fresh growth is refused with the existing
typed `document_capacity` response. Unknown baseline or unavailable accounting
fails closed through the existing storage-unavailable response.

Reads, history, title-only changes, sharing and owner grant/link revocation do not
reserve source storage. Exact committed metadata retries precede quota admission.
Runtime publication revalidates authority before reservation and after upload.
This change is source accounting, not completion of document deletion, orphan
cleanup or the separate #1480 retained-data deletion acceptance.

## Durable format and bounds

Agent Manager owns `cma_document_quotas`, added to its exact schema/role registry
and startup version guard even while the feature is off. Rows use schema version
1 and the existing `_id` index: one activation marker, one owner aggregate, one
document reservation per owner/document and one object reservation per
owner/document/digest. Owner identifiers are SHA-256 digests. Object rows retain
only the charged length; no source text, title, credential or grant is copied.
Existing document, revision, receipt, object-key and storage-generation formats
are unchanged. No application collection or object is deleted or rewritten.

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

This is an additive expand/install/enforce migration. Existing data/recovery units
remain intact and no destructive transform needs a new pre-delete backup. Retain
the private inventory digest, writer/drain receipts, dry-run result, aggregate
import progress, ready state, deployed source/digest/config and compatible rollback
receipt. Startup collection creation/version checks have no new wall-time bound;
qualify readiness/index/role behavior before the activation window.

Rollback uses the qualified compatible image and retains all ledger rows. Setting
the flag false deliberately blocks new source writes after preparation; it never
refunds uncertain capacity or restores unaccounted legacy writing. Read/revoke and
committed metadata replay remain available. Recovery is forward-only when the
baseline is incomplete or uncertain. Storage-owner authorized orphan cleanup and
any future reservation release must prove object finality and preserve exact
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
