# CMA data migrations

This is the cross-store migration standard for CMA. It supplements the binding
[repository contract](../CLAUDE.md), [delivery authority](CMA_Operations.md#delivery-authority)
and [service ownership rules](CMA_Service_Isolation.md). It grants no production
write authority. Existing continuity, security, isolation and CI requirements
remain binding. Historical reset/cutover procedures do not authorize repeating
a reset on the current Agent model.

A data change includes a new nested event variant, nullable field, serializer,
index, validator, TTL, default, background writer, file/journal layout, encrypted
envelope, external receipt, client journal or configuration interpretation.
An unchanged Mongo envelope version and absence of a manual migration command
are not evidence that the durable format is unchanged.

## Required delivery sequence

1. **Expand.** Introduce compatible readers and additive structures while new
   writers are closed. Inventory every writer and consumer, including ordinary
   requests, workers, startup DDL, importers, retries, old deployed processes,
   runtime homes and retained browser assets. Identify strict readers and whole
   record replacements that could reject or discard new fields. Do not weaken
   validation to disguise an unsupported format.
2. **Deploy compatible readers.** Verify all serving and recovery readers,
   workers and migrators against retained old data and the planned new shapes.
   Record immutable images and source identities. Drain incompatible processes
   before enabling writers; a rolling overlap is not automatically safe. Keep
   retained runtimes on their recorded images and homes. A new runtime default
   cannot authorize replacement of an active Sandbox.
3. **Enable gated writers.** Use an explicit, default-closed admission boundary
   tied to the affected writer and authority/cohort. Check ordinary writes and
   background enqueue/claim/generation, not just UI or request admission. Stage a
   bounded cohort and record observations before expansion. Reads, exact accepted
   retries, uncertainty resolution and already-admitted operation settlement must
   remain available when admission closes. Document any finite recovery writes
   that may continue after closure and their reader requirements.
4. **Backfill.** Run only the reviewed bounded transformation after its readers
   and prerequisites are present. Preserve original IDs, versions, request keys,
   provenance, encryption custody, leases, runtime pins and external-effect
   receipts. Checkpoint progress, use per-record CAS/fences, throttle load, and
   retain errors and skipped records for review. Never infer completion from
   activation, a successful deployment, or one empty scan.
5. **Contract.** Retire an old reader/writer/index/format only in a separately
   reviewed step with evidence that no retained data, old client asset, pending
   operation, leased worker or recovery image still needs it. Confirm the
   retention window, backup/restore rehearsal and post-write recovery plan.
   Contract is an irreversible boundary unless a tested inverse preserves every
   intervening accepted write. A flag flip is not such an inverse.

Phases may span releases. Each release states its phase and exit criteria; a
small additive change may document a phase as unnecessary with evidence. No
phase may be skipped merely because the records use schema version 1.

## Ownership and recovery units

The implementing PR names a responsible service owner and operator for each
store. Shared credentials do not permit cross-service collection access or
transactions. Cross-store ordering uses durable receipts and reconciliation;
Mongo cannot atomically commit a PVC mutation, registry write or NyxID effect.

| Store / authority | Owner and current mechanism | Preserve and verify |
| --- | --- | --- |
| Agent Manager Mongo catalog, logs, commands, feedback, suggestions, profiles, settings, usage and provenance | Agent Manager; catalog in `api/crates/cma/src/store/isolation.rs`, initialization in `store/mongo.rs`, typed codecs and bespoke backfills | IDs, event order, command/retry identities, ownership, revisions, leases, reader semantics and unique constraints |
| Agent Manager auth/custody and activity | Agent Manager; encrypted envelopes and TTL/index initialization | Encryption key identity/custody, deployment identity, token receipts, authorized retention and TTL effects |
| Gateway `eg_*`, optional stores, sessions and sealed records | Event Gateway; explicit `cmaeg --migrate`, optional commands and flag-controlled background GitHub initialization in [Gateway Operations](CMAEG_Operations.md#native-github-startup-initialization) | Exact sealed bytes, master keys/key IDs, CAS revisions, pointers, uncertain-effect fences, expiry and independent schema markers |
| Image Manager `im_*` and build receipts | Image Manager; explicit `cma-image-manager --migrate`, startup verification | Catalog/shares, request identity, immutable successful image digests, schema/index expectations and auth custody |
| Persistent Sandbox homes, file layouts, journals and browser/desktop profiles | Layer 4 owns supervisor formats; infrastructure controller owns native PVC/Claim/Sandbox resources; Agent Manager retains allocation authority | Same home/PVC UID, original claims, dirty work, native conversations, runtime pins, atomic publication and restart semantics |
| Provider SQLite/conversation state inside homes | Pinned provider binary owns its formats; Layer 3 negotiates supported behavior | Provider version and same-home recovery; CMA does not rewrite provider databases through a generic SQL migration |
| Document object bytes and retained checkpoint/archive objects | Agent Manager owns metadata/revisions; storage service owns object durability; historical checkpoint implementation is retired | Object version/hash/size, metadata-to-bytes consistency, custody and original references; retained archive readability is not proof bytes still exist |
| OCI registry and Cloud Build state | Image Manager owns application receipts; registry/build infrastructure owns remote artifacts | Digests and build IDs, referenced runtime images, idempotent reconciliation and retention of admitted references |
| ConfigMaps, Secrets and Kubernetes resources | Authorized operator/infrastructure owner, with service parsers and exact resource identities | Observe live state; use scoped changes preserving non-target fields, key custody and resource versions; never apply whole checked-in templates over live configuration |
| Browser/SDK retained journals and static assets | Web/SDK format owners; operator retains hashed asset sets | Account/Agent scoping, original uncertain-input keys, drafts, transport choice, cursors and old/new assets through refresh/recovery |
| NyxID, provider messages/assets and billing effects | Respective external service owns authority; CMA/Gateway owns its local receipt and reconciliation | Original remote IDs, credentials' custody, callback origins, request/generation/fence identity; never duplicate an uncertain effect to simulate rollback |

A release may affect only one row; its recovery unit can still include several.
For example, restoring encrypted Mongo metadata without the matching keys, or
home metadata without the original PVC, is not a usable restore.

## Migration identity, ordering and execution

Every data-affecting issue/PR carries a version-controlled migration record in
Markdown under `docs/`, or a named section of its release runbook. Record:

- An immutable ID, for example `2026-11-05-am-feedback-writer-admission`, service,
  owner, issue/PR, purpose, phase, source commit/tree and exact executable/image.
- A checksum of the reviewed executable/manifest or transformation, source and
  target durable format IDs (including nested variants), expected prior schema,
  indexes, validators and TTLs. A mutable CAS revision is not a schema version;
  a component's current marker is not an execution history.
- Explicit prerequisite migration IDs and reader deployment/drain receipts.
  Order by dependencies, not timestamp alone. Refuse unknown, conflicting,
  newer-than-supported or checksum-drifted state; never let an older migrator
  silently replace a newer validator or completion marker.
- Execution identity, start/end, outcome, attempted/scanned/changed/skipped/error
  counts, cohort/cursor, interruption/resume state, verification receipts and
  remaining exceptions. Keep append-only attempt history; do not overwrite a
  failed attempt with a success label.

Use a service-scoped renewable execution lease with a fencing token where an
executor exists. Check ownership before every batch/effect and stop on lease
loss; expiry alone does not authorize an old worker. Per-record CAS remains
necessary under a global lock. DDL and its marker are not one transaction:
preflight expected state and design recovery for each partially applied step.

Until the runner/ledger follow-up exists, the release record must name the
current component command, its actual concurrency guarantees, and how the
operator excludes concurrent execution. A manual single-operator procedure is
not a durable fenced lock. If the available mechanism cannot safely meet the
step's concurrency/restart requirements, defer that step rather than claim the
missing mechanism exists.

Idempotency means a rerun preserves the exact completed result and refuses a
conflicting result. Use immutable insert/readback or guarded CAS as appropriate;
retain original retry identities through ambiguous commits. Test interruption
before/after every durable boundary, rerun after partial DDL, simultaneous
execution and stale lease holders. External effects require their own receipts,
reconciliation and compensating procedure; no new key on an uncertain retry.

## Reader floor and rollback record per release

The reader floor is the minimum **implementation and recovery behavior** able
to interpret every retained shape and finish every admitted operation. Record
actual immutable API/Gateway/runtime/client artifacts, not just a tag, envelope
version or successful deserialize. The permitted recovery set must cover all
writers in the release batch, including code merged from other PRs.

The release record must include this table, with `not affected` and a reason
where appropriate:

| Required field | Evidence |
| --- | --- |
| Release / migration identity, phase and owners | Issue/PR, source commit/tree, operator and store owners |
| Affected stores and all writer triggers | Ordinary, worker, startup/import, retry/recovery, client/file/external paths |
| Old/new format and preservation contract | Nested fields/variants, indexes/TTL/validator changes, original identities |
| Compatible reader floor / allowed recovery artifacts | Exact image digest or hashed executable/assets, source identity, semantics retained, tested combinations |
| Writer gate and cohort | Exact flag/capability, default, authority selection, all process versions, closure/drain evidence and recovery exceptions |
| Backfill execution | Command/checksum, prerequisites, locking/CAS, bounds, progress, retry and completion receipts |
| Backup and dry-run | Protected backup IDs and timestamp, recovery-unit coverage, rehearsal result, restore owner and post-backup write handling |
| Rollback / forward recovery | Exact procedure and compatible digest, point of irreversibility, retained runtime/asset/key dependencies, abort criteria |
| Verification and completion | Local/CI evidence, production observations and limits, unresolved exceptions, reviewer/operator sign-off |

Test old data with new readers, and new data with every permitted recovery
reader. Include policy-disabled ordinary traffic, replay, restart, worker
handoff, permission failures and exact retries after closure. A negative test
showing a predecessor refuses new data excludes that predecessor from recovery;
it does not certify it. Test semantic preservation as well as decoding.

The matrix distinguishes historical, current, candidate and explicitly approved
recovery executables. Each cell declares the fixture/cohort, writer policy,
expected admission/refusal and recovery outcome, exact source SHA and artifact
digest, environment, time window, sample count, configuration and raw-result
location. Missing, cancelled or unexecuted cells remain unverified; a retry that
passes does not erase the original failure. A model of an older decoder is useful
regression coverage, but only execution of the pinned older artifact proves that
artifact. Test refusal before traffic or writes, not just an eventual parse error.

The [November 4 task-experience floor](plans/cma-ui-optimization/19-COMPATIBILITY.md)
is a concrete example: tool output and expanded feedback crossed the predecessor
API boundary outside task admission. The #1311 reader reference is API digest
`sha256:b0499635ea0345e513d2032108236be449787f144e1247d553647b2307c08756`;
it is not a blanket recovery certificate for later batches. Writer closure
limits future changes; it never removes the floor imposed by retained data.
Prefer a forward fix with compatible readers. Never delete/downgrade records,
reset a database, clear journals or replace an active home to fit an old image.

## Startup, strict readers and schema guards

The November release lessons apply to every component:

- **Optional initialization is still migration.** Gateway GitHub initialization
  runs after the listener binds, with a 30-second bound and capability-specific
  readiness. The base readiness probe succeeding does not prove that migration
  or the GitHub capability is ready. Capture the schema/index postchecks and
  capability result; preserve ordinary channel availability on optional failure.
  A later startup may resume partial work. Shared-schema credentials do not grant
  another service ownership or authorize new grants.
- **Additive fields can raise the reader floor.** An older
  `deny_unknown_fields` decoder can reject `guard_revision` or a sealed profile's
  `queue_lifetime_seconds` even when the outer schema version is unchanged.
  Disabling the producer does not remove retained fields or admitted work.
  A metadata-only scan cannot prove absence inside encrypted profiles. Without
  complete authorized evidence, that predecessor is ineligible; retain a
  compatible forward-repair artifact. Never unset fields, rewrite sealed profiles,
  drop indexes or run an older migrator to make its reader fit.
- **Guards must match actual writer bytes.** The
  [session schema audit](CMA_Session_Schema_Audit.md) records a retained lease
  whose writer omitted the version required at the next boot. Exercise the real
  serializer against the startup predicate, including integer BSON variants,
  missing/wrong-type/unknown versions and retained known legacy shapes. Keep
  compatibility exceptions exact and read-only; refuse corruption without
  silently repairing it. Diagnostics contain aggregate counts and reason codes.

Inventory startup collection creation, index builds/drops, validator changes,
TTL changes and whole-collection scans. For each, name its service, trigger,
expected prior state, maximum work/time, partial-failure recovery and receipt.
Move nontrivial DDL into reviewed planned steps before replacing startup with
bounded read-only checks. Keep the currently required schema until that cutover
is proven. In particular, an activity TTL mismatch is a retention decision, not
permission for an arbitrary create-index failure to trigger `collMod`.

A lease around DDL is not by itself server-side fencing: an already submitted
command may finish after its executor loses ownership. Declare which steps are
safe under that overlap, verify exact prior/postconditions, and require exclusive
execution or another proven boundary for destructive steps. An older migrator
must refuse newer state before any write, not after replacing its validator.

For [M-OPS-02](https://github.com/ChronoAIProject/cma/issues/1554), retain separate
evidence for TC-089 (reader/writer matrix), TC-090 (bounded startup and migration
resume) and TC-091 (compatible recovery). On the declared 90-day dataset, measure
at least ten startups per migration stratum and require each maximum below 60
seconds. Report p99 only with at least 1,000 samples in that same stratum. Record
two compatible recovery drills, each at most 30 minutes from first failure to
five minutes of stable probes. The qualification dates are November 19 and 20,
2026, before the November 23 gate; later freeze rechecks do not replace them.
Document/CI merge is not evidence that these production criteria have passed.

## Backup, dry-run and irreversible steps

Before a destructive transform, index/validator contraction, TTL reduction,
key retirement or deletion, require a reviewed protected backup and a dry-run
against a disposable copy. The backup must be restorable with matching keys,
metadata, home snapshots/object versions, provider versions and external receipts.
Record the consistency boundary, timestamp, retention and access controls,
restore operator, restore duration and completed rehearsal. Retain key IDs and
custody references in evidence, never secret values or user message bodies.

The dry-run reports scope/counts, expected transformations, conflicts, size/load,
index constraints and exceptions without changing production. Validate both the
forward result and recovery using the exact candidate artifacts. A restore
plan must preserve or reconcile writes accepted after the backup; restoring a
stale snapshot over live data is not rollback. If a coherent recovery unit or
post-backup-write plan cannot be proved, do not perform the irreversible step.

For a gate-only change with no transformation/deletion, explain why no backup or
backfill is needed; still record the retained-data recovery floor. Production
verification is scoped to authorized actions. Read-only observations are not
claims of successful production migration, and neither CI nor documentation
implicitly authorizes a production test turn or write.

## Completion and current enforcement

Use the [PR checklist](../.github/PULL_REQUEST_TEMPLATE.md) for each data change.
All existing lint, typecheck, tests, Mongo contracts, source-size/structure,
secret, image and manifest gates remain required. Do not weaken an existing
assertion or bypass a gate to make a migration pass. Repository tests establish
only their stated coverage; public-wire compatibility alone cannot establish
storage compatibility for a previous API executable.

The release owner closes the migration issue only after production verification
records all exit criteria and unresolved exceptions are assigned. A merged PR,
green CI, deployed command or enabled cohort alone is not completion. Backfill
completion needs reconciled counts, review of skipped/errors, stable invariants
and evidence that no remaining eligible records or dependent consumers need the
old contract.

This standard adds review requirements, not a claim of universal automation.
Follow-ups under [CMA data migration safety](https://github.com/ChronoAIProject/cma/milestone/23):

- [Reader/recovery compatibility CI matrix #1327](https://github.com/ChronoAIProject/cma/issues/1327).
- [Service migration runner/ledger #1328](https://github.com/ChronoAIProject/cma/issues/1328).
- [Planned startup DDL #1329](https://github.com/ChronoAIProject/cma/issues/1329).
- [Backfill progress and completion surfaces #1331](https://github.com/ChronoAIProject/cma/issues/1331).

## 2026-11-13-am-document-source-commit-state

This release adds the `state` (`reserved`, `committed`, `condemned`) and
`reserved_at` fields to `cma_document_quotas` object rows and makes source
metadata commits atomic with the `reserved` to `committed` transition. The
migration is additive and default-closed. Its reader floor is the identity-only
quota comparison reader: that reader must first be deployed and pinned as the
rollback target before any writer can emit the new fields. Rows from older
writers without `state` are interpreted as `reserved`; they are not rewritten
by this release and become `committed` only when a later metadata commit proves
the reference. The reclaim worker introduced later owns the `condemned`
transition.

The affected writers are the document service create and Markdown update paths,
the Mongo document store transaction helper, and the quota baseline importer.
The transaction writes the expected metadata and the object state together;
CAS conflicts abort without changing the object, and condemned objects abort
without changing metadata. Unknown commit results are reconciled at the
recorded operation time. Keep the compatible reader image, Mongo backups and
activation manifest as recovery artifacts. Completion evidence includes exact
retry identity, legacy-row reads, commit/CAS conflict behavior, condemned-row
refusal and the per-process 16-slot busy refusal.

Exact identity retries refuse an object row already marked `condemned` before
upload, leaving its counters unchanged so reclaim cannot be followed by a late
PUT that recreates an unledgered object. Rows in `reserved`, `committed`, or the
legacy no-`state` form retain the free retry behavior.

The metadata commit and its bounded retries have a three-second overall
deadline. The deadline can expire after `commitTransaction` has been sent, so
the caller returns `Unavailable` when the final result is uncertain. The same
create idempotency key or update receipt is the recovery path for reconciling
that outcome.

## 2026-11-13-am-document-reclaim

- **Service, issue and phase:** Agent Manager, #1479 PR-5; additive expansion and
  gated reclaim. Release owner records the actual merge/source SHA, executable
  checksum, immutable image and writer census before activation. Prerequisite:
  `2026-11-13-am-document-source-commit-state` and the separately scoped storage
  maintenance client. This record grants no production deletion authority.
- **Stores and formats:** existing `cma_document_quotas`, schema version 1;
  startup adds the non-unique `{kind: 1, state: 1, reserved_at: 1}` index through
  the existing document-store index initializer. No collection or TTL is added.
  The reclaim judgement writer conditionally changes reserved or missing-state
  object rows to `committed` or `condemned`; new object rows also receive a unique
  `reservation_id`. Judgement, finalization and unknown-commit readback match that
  incarnation so old candidates cannot release a fresh same-key reservation.
  Each condemned-row deletion additionally claims `delete_holder` (unique worker
  startup ID), `delete_fence` (incremented on every claim) and
  `delete_lease_until` (five minutes). A missing lease is claimable; a present
  lease is claimable only after strict expiry. Finalization must match both holder
  and fence as well as the reservation incarnation, so an expired holder cannot
  release a row after takeover. Missing fields on legacy condemned rows require
  a first claim before deletion; no backfill or new index is needed.
  Legacy rows match only a missing reservation ID. Condemnation writes server
  `condemned_at`. New baseline owner rows add immutable `baseline_documents`,
  `baseline_bytes`, `baseline_objects`; the ready transition adds server
  `activated_at`. Old partial imports retain missing baseline fields instead of
  fabricating original usage. Legacy object age uses activation only when it is
  present; unknown-age rows require manual investigation.
- **Reader floor and gates:** every source writer and the qualified rollback
  image must contain PR-3's identity-compatible quota reader and atomic source
  commit marker. Active reclaim additionally requires this release's
  reservation-ID writer on every reserving process; with reclaim disabled,
  rollback retains the PR-3 commit-marker floor. Old writers that do not mark
  object commits are incompatible with reclaim and must be drained before
  activation; a grace period is no
  substitute. Retain the all-writer census and drain proof. Reclaim requires
  `CHRONO_DOCUMENT_RECLAIM_ENABLED=true`, enabled ready quotas, and proved
  maintenance deletion support. All feature switches remain default-off in code
  and manifests. E1/E2 storage/finality/scope evidence is still external.
- **Migration and work bounds:** existing index creation is idempotent, outside
  user requests and readiness retries; it has no new DDL completion guarantee.
  Candidate queries have a three-second maximum, indexed state selection, and
  at most 64 rows. Startup waits 30 seconds; each five-minute round uses the
  shared background budget with a 30-second deadline, no queued admission and
  at most four external deletions. Failed deletion preserves condemned rows and
  respects dependency retry delay. No transaction contains storage I/O.
  Every deletion claims independently, including newly condemned rows. It starts
  only when the remaining lease exceeds the 30-second storage operation budget
  plus a five-second margin. Failed, cancelled or short-budget claims remain
  leased until expiry; they are never immediately unlocked for another delete.
- **Counter writers and recovery:** finalization fences the same owner row as
  reserve, removes only a condemned object row, and subtracts bytes/objects only
  for the transaction that removed it. A missing main document plus no remaining
  object rows allows document-reservation removal and documents decrement.
  Any negative result aborts the whole transaction as corruption. Shared
  transaction helpers reconcile unknown commits using a snapshot at operation
  time; ambiguous results retry without inventing a release. Process metrics
  observe work and do not replace durable accounting.
- **External finality and authorization:** automatic deletion covers only
  ledgered objects proved unreferenced in the judgement transaction. The deletion
  lease serializes duplicate deletes by compatible reclaim workers. Drain older
  reclaim workers before activation. The remaining E1 item 5 contract must prove
  an abandoned DELETE cannot land after lease expiry and remove a new generation
  after same-key reuse; local holder/fence checks do not prove remote finality.
  Outside-ledger cleanup requires private report review, ready ledger, proved
  writer quiescence, dry-run and explicit CLI apply; apply rechecks current
  ledger/reference absence. The owner-scoped listing interface cannot attest a
  complete global physical inventory. Retain the independent storage-owner
  inventory and version/soft-deletion evidence.
- **Recovery and backup:** retain the private baseline inventory and digest,
  pre-activation Mongo/object backup, all-writer/drain evidence, storage contract,
  report digests, aggregate CLI receipts, source/image/config and compatible
  rollback receipt. Reconciliation is read-only, checkpointed within a 60-second
  call, and incomplete or unsupported checks are `NOT_CHECKED`. Reports contain
  hashes/keys and aggregates, never subjects, titles or source text.
- **Rollback:** disable `CHRONO_DOCUMENT_RECLAIM_ENABLED` and restart to stop new
  rounds. Preserve the index, activation and ledger rows. A closed switch cannot
  restore objects already deleted. Restoring any object/reference/accounting
  unit needs an independently verified recovery procedure; do not reset owner
  counters or re-enable pre-PR-3 writers.
- **Completion:** local unit and disposable-replica-set tests must verify
  writer/judge serialization, exactly-once release across stores, reserve/finalize
  owner fencing, condemned retry, negative-count refusal, indexed selection,
  report invariants/resume/privacy and cleanup refusals. Production completion
  remains blocked on E1/E2/E3, approval of automatic cleanup, qualified backups
  and actual deployed writer/storage evidence. Do not substitute fixture PASS
  for provider or production acceptance.

## 2026-11-13-am-document-lifecycle-and-purge

- **Service, issue and phase:** Agent Manager, #1479 PR-6; additive expansion,
  default-closed deletion and purge writers. Prerequisites are
  `2026-11-13-am-document-source-commit-state`,
  `2026-11-13-am-document-reclaim` and the separately scoped maintenance client.
  The release owner records source SHA, executable checksum, image digest,
  writer census and deployment/config receipts before activation.
- **Stores and writers:** new Agent Manager collection
  `cma_document_lifecycle`, `document_schema_version: 1`, keyed by document ID;
  indexes `{state: 1, "purge.next_at": 1}` and `{owner_hash: 1}`. Startup schema
  creation/version guard, exact isolated grants, shared-schema authority and the
  retirement census include this collection. Compatible create writes `live` in
  the metadata/source-commit transaction while deletion is enabled. Owner DELETE
  removes `cma_document_artifacts` and writes `deleting` plus deletion receipt
  hashes, revision-object snapshot and purge schedule in one transaction.
  Existing document quota rows become `deleting`; an existing owner fence
  advances. Neither deletion nor unledgered purge upserts quota rows.
- **Formats and privacy:** lifecycle stores `owner_hash`, `state`, `created_at`,
  `deletion{deleted_at, revision, receipt_key, fingerprint}`, at most 256 `objects`
  keys and `purge{pass, next_at, attempts, lease{holder, fence, expires_at}, blocked,
  completed_at}` plus optional `purge.position` for bounded batch/page progress
  and `purge.last_reserved_at`, the latest reservation timestamp captured under
  the deletion transaction's owner fence. Persisting that timestamp avoids a
  complete ledger scan on every four-object wave.
  Missing position starts the current pass; the position holds only object keys
  and opaque bounded listing cursors. It contains no source, title, subject, comments, shares or
  original receipt body. The main document envelope/codec is unchanged; old
  readers see an absent main row. Tombstones have no TTL and document IDs are
  never reused.
- **Reader floor and gates:** before opening
  `CHRONO_DOCUMENT_DELETION_ENABLED`, **all API processes** must run this version
  and this version must be the pinned rollback target. An older create writer
  does not check tombstones and can resurrect a deleted ID with the same
  idempotency key. Deploy with switches false, prove all-writer convergence and
  drain, then enable deletion uniformly. Compatible flag-off create performs
  one tombstone point read before its plain insert. These two operations are not
  atomic: an OFF create that passed its read can insert after a racing ON delete.
  Drain every OFF create request before admitting deletion and never mix ON/OFF
  writers during deletion admission. Flag-on create atomically inserts the
  unique lifecycle identity. No non-document request gains a database read.
  Quota, reclaim and storage capability gates remain independent and default-off.
- **Migration and work bounds:** no backfill or destructive rewrite is needed.
  Existing documents acquire lifecycle on deletion; only new gated creates write
  `live`. Each background step handles one document under the shared 30-second
  work deadline with a 90-second holder/fence lease and at most four deletes in
  flight. A durable substep lists one page or deletes one wave of at most four
  keys, then saves its position, preventing repeated timeout from starving later
  objects in a large document. Every state write checks the unexpired lease. Pass two waits until the
  last reservation/deletion plus ten minutes and lists again to catch late PUTs.
  A 429 retains the current step and schedules retry. Storage calls stay outside
  database transactions. Unsupported deletion persists blocked and never releases
  accounting or claims completion.
- **Counter release and recovery:** the purge release transaction serializes with
  reserve through the existing owner fence, removes all actual document object
  rows and its document reservation, validates nonnegative counts and decrements
  only those rows, then marks lifecycle `purged`. Unknown commits reconcile from
  a snapshot at operation time. Replays and competing stores release once.
  Reclaim yields lifecycle-owned objects and cannot release a document slot while
  any lifecycle exists. Discovery excludes purge-owned rows through indexed
  lifecycle identity lookups before limiting its output to 64 candidates; the
  three-second deadline bounds rejected scans, whose row count can exceed 64.
  With no quota rows, purge only completes lifecycle.
- **Recovery and backup:** retain the private inventory, Mongo/object recovery
  unit, storage finality/scope evidence, all-writer census, enabled-switch record
  and qualified rollback image. A backup restore must preserve tombstones and
  deletion progress so restored metadata cannot revive deleted IDs. A flag cannot
  restore removed objects. Do not roll back below this reader floor or delete a
  tombstone to repair a failure. The target is one-hour purge; alert after
  24 hours and escalate before seven days. E1/E2 absence keeps physical cleanup
  blocked and prevents production deletion acceptance.
- **Cross-plan dependency:** `origin/develop` includes #1500's Web design but no
  account-erasure directory implementation at this change's baseline. #1500 must
  add `cma_document_lifecycle` to its directory with owner-hash scope and object
  ledger classification when that directory lands, preserving its CI equality
  with `isolation::COLLECTIONS`.
- **Completion:** verify owner-only immediate loss of every access path,
  stale-revision/idempotency failures, flag-off refusal, late-create rejection
  with either flag value, reserve/delete serialization, two-pass late-PUT cleanup,
  exactly-once release across stores, expired-holder refusal, pending usage and
  old-codec absence against the disposable replica set. Fixture success does not
  replace storage-owner finality evidence or authorize production activation.
