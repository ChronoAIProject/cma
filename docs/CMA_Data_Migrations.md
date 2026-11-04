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
| Gateway `eg_*`, optional stores, sessions and sealed records | Event Gateway; explicit `cmaeg --migrate` and separately scoped optional commands in [Gateway Operations](CMAEG_Operations.md) | Exact sealed bytes, master keys/key IDs, CAS revisions, pointers, uncertain-effect fences, expiry and independent schema markers |
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

The [November 4 task-experience floor](plans/cma-ui-optimization/19-COMPATIBILITY.md)
is a concrete example: tool output and expanded feedback crossed the predecessor
API boundary outside task admission. The #1311 reader reference is API digest
`sha256:b0499635ea0345e513d2032108236be449787f144e1247d553647b2307c08756`;
it is not a blanket recovery certificate for later batches. Writer closure
limits future changes; it never removes the floor imposed by retained data.
Prefer a forward fix with compatible readers. Never delete/downgrade records,
reset a database, clear journals or replace an active home to fit an old image.

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
