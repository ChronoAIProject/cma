# S4 atomic Agent-log writer interface — 2026-11-11

Part of #1350 and transactional #1349. This is the implementation contract for
`tl/cma-log-segments/writer-publication`, following the published five-slice plan.
It fixes the integration boundary; executable acceptance is still required.
The earlier `writer` assessment branch and old integration remain preserved.
Develop merge freeze remains in force. No flag or production write is enabled.

## Ownership and common base

S1 owns durable Head/Seal/envelope codecs, captured source proof, range readers,
bootstrap source validation, staged audit/rebuild, schema and segment registry.
S2 owns shared trait/root registration/config/migration/grant/CI plumbing and
integration. S3 owns read adapters and its title/trigger insertion callers. S4
owns projection/write/receipt schemas, publication planner/transaction helper,
gate/control, and the assigned admission/command/conversation/worker/lifecycle
callers. S5 owns recovery/domain checks and its checkpoint/reconciliation callers.
No owner should duplicate another owner's helper or edit their registry.

Base: current develop plus `integration-20261111` by normal merge. The first
contract publication uses the plan commit `7b4b1a779` and develop `4b98d4232`.
The six historical writer drafts are ported without importing the old reader
merge-resolution ancestry. All source and tests touched by S4 stay under 500 lines.

## Durable format shared with S1

Reuse `4db1c7cfc`'s `codec.rs`, `digest.rs`, and `envelope.rs` contract. S1 remains
the implementation owner. Schema changes require a coordinated version change;
there is no second writer-specific head or seal schema.

- Original `agent_log` documents, `_id`, query fields, global `seq`, raw BSON and
  public cursors are retained. The unique `(agent_id, seq)` index remains.
- Legacy Agent envelope is version 1 with no segment markers. Version 2 requires
  all three markers: `agent_log_format = "segmented_v1"`, immutable
  `agent_log_incarnation`, and signed `agent_log_head_revision`. A partial,
  unknown or inconsistent envelope is corruption, never a legacy fallback.
- Incarnation is the historical `sha256` creation-identity encoding; rollover
  never changes Agent creation/native/home identity or public incarnation.
- `Head` is in `agent_log_heads`, keyed by Agent ID. All counter/revision fields
  are BSON int64; all increments and serialization conversions are checked.

| Head fields | Contract |
| --- | --- |
| `_id`, `format_version`, `incarnation` | Agent key, format 1, immutable creation binding. |
| `revision` | Monotonic head CAS; every segmented publication advances it. |
| `active_ordinal`, `active_start_seq`, `last_seq` | Scalar current segment interval and global through. Empty active interval is start = last + 1. |
| `active_count`, `active_bytes` | Exact current segment event count and complete BSON bytes. |
| `previous_seal` | Explicit null for ordinal 0; otherwise prior seal's 32-byte generic BSON Binary digest. |
| `lifetime_count`, `lifetime_bytes` | Checked cumulative counters; count equals global last_seq. No lifetime capacity policy. |
| `projection_format`, `evidence_generation`, `projection_through` | Versioned derived state binding; through equals last_seq; no growing arrays in Head. |
| `migration_id`, `required_reader` | Existing reader-format constants from S1, not the writer migration ID. |

`Seal` is immutable in `agent_log_segments` and uses the existing
`Seal::identity(agent, incarnation, ordinal)` encoding. Its exact fields are
`_id`, `format_version`, `agent_id`, `incarnation`, `ordinal`, `start_seq`,
`end_seq`, `entry_count`, `encoded_bytes`, `previous_seal`, `prefix_count`,
`prefix_bytes`, `leaf_digests`, `seal_digest`, `digest_version`,
`sealing_operation`, `source_format`, and `storage_kind`.

The existing `sha256_raw_bson_v1` domain/length encoding is authoritative.
`leaf_digests` concatenates the original rows' 32-byte digests in global sequence
order; no reserialization is allowed when hashing retained rows. `seal_digest`
covers all historical manifest fields using the existing codec. Exact serialized
manifest BSON is <= 1 MiB. Unique Agent/incarnation ordinal and start indexes,
predecessor linkage and cumulative bounds prohibit gaps/duplicate successors.
There is no empty seal and no TTL or deletion of sealed source/history.

| Capacity class | Entries | Complete BSON bytes |
| --- | ---: | ---: |
| Normal | 10,000 | 64 MiB |
| Stop/failure | 10,064 | 64 MiB + 512 KiB |
| Delete/physical | 10,128 | 65 MiB |

Each event remains <=16 MiB and each proposed batch <=64 events. Encode the
entire batch before deciding whether to roll. A batch unable to fit an eligible
empty segment fails without changing any document. Normal overflow seals the
nonempty active segment and puts the entire batch in one successor. Segmented
terminal cleanup cannot create an endless sequence of reserve-only segments.
The global cursor limit is 9,007,199,254,740,991 with checked int64 counters.

## Publication identity, receipt and outcomes

`publication::PublicationIdentity` is the shared operation identity. Callers
supply `key: String`, `fingerprint: Binary` (generic 32-byte digest), and stable
`appended_at: String` before the retry loop. Fingerprint is a domain-separated,
canonical digest of the original logical request and immutable bindings, not
of mutable head/lease state. Fresh lease holders do not invent new keys or times.
Different legitimate keys with equal payloads remain distinct operations.

Use existing durable command/work/lifecycle identities where available. Bare
append obtains one identity before retry and preserves it with its exact
publication attempt. The existing unkeyed trait remains legacy compatibility;
S2 supplies the keyed port needed by durable callers. No process-restart caller
may silently allocate a replacement key for an uncertain operation.

`agent_log_publications` contains an immutable completed receipt, keyed by
`publication::identity(agent, incarnation, operation.key)` with a unique
`(agent_id, incarnation, publication_key)` index. Preserve the historical fields:
`format_version`, `agent_id`, `incarnation`, `publication_key`, `fingerprint`,
`expected_revision`, `expected_seq`, `first_seq`, `last_seq`, `result_revision`,
`head_revision`, `active_ordinal`, `previous_seal`, and `result_digest`.
The writer additionally records the stable `appended_at` with the receipt.
`result_digest` binds the exact caller result and related effect/receipt proof;
it is distinct from the request fingerprint. Metadata-only publication has
`first_seq = expected_seq + 1` and `last_seq = expected_seq`, an empty interval.

`agent_log_admissions` is separate: it proves that one exact request was accepted
under a particular gate revision. It never proves publication or external
execution. Reuse its historical Agent/incarnation/owner/key/fingerprint/source
revision/sequence/time/kind/gate-proof fields. Exact timestamp is compared too.
Admission kinds are Activation, Rollover and CapacityRecovery; recovery requires
both independent gate proofs. S1/S2 register admissions and gates in schema,
grants, migration, backup and restore inventories alongside the six sidecars.

Reconciliation outcomes are explicit:

| Outcome | Required proof / caller action |
| --- | --- |
| Recorded | Exact immutable receipt + request/result binding + original event interval + manifest linkage + related receipt evidence in one coherent majority-visible snapshot. Return the original result, including after later appends. |
| KnownAbort | Transaction outcome proves abort. Same key/fingerprint/time may replan against fresh current fences. |
| Unknown | Missing or not-yet-visible receipt, unresolved commit, or unavailable proof. Retain the attempt; do not resend native input, acknowledge a checkpoint, settle capacity, or create a new successor. |
| Conflict / corrupt | Reused key with changed fingerprint or invalid identity/source proof. Fail explicitly; do not reinterpret it as absent input or capacity. |

Reuse `sandbox_mongo::commit_transaction` and its commit retry semantics. The
publication helper never starts, commits or retries the caller's transaction.
An uncertain commit is reconciled before any fresh publication attempt. A
matching receipt is checked before comparing current aggregate equality; later
head advancement does not invalidate the earlier exact outcome.

## Same-transaction API boundary

Implementation lives in `agent_log_segments/writer_commit.rs`, with pure
planning in `writer_plan.rs`, bootstrap in `writer_bootstrap.rs`, projection
updates in `writer_projection.rs`, and reconciliation in `writer_reconcile.rs`.
The contract names below are mapped onto existing types, without a parallel
transaction framework. S1/S2 publish the required registry/shared ports.

```rust
// Public within crate::store; fields are validated, not caller assertions.
struct PublicationInput<'a> {
    operation: &'a PublicationIdentity,
    current: &'a Agent,
    next: &'a Agent,
    logs: &'a [AgentLogEntry],
    capacity: AppendCapacity,
    result_digest: Binary,
}

// S1 supplies the captured source proof. This context is obtained once in the
// caller's transaction, then reused for semantic validation and planning.
async fn prepare(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    captured: &CapturedLog,
    input: PublicationInput<'_>,
    bootstrap: Option<&BootstrapPlan>,
    admission: Option<&Admission>,
) -> Result<PreparedPublication, TransactionLogError>;

// The caller builds its existing domain/fence predicate and replacement
// fields; the helper owns the single Agent CAS and adds strict captured-source
// and format conditions. No independently applied Agent replacement is needed.
struct AggregateUpdate {
    filter: Document,
    fields: Document,
}

async fn persist(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    prepared: PreparedPublication,
    aggregate: AggregateUpdate,
) -> Result<PublicationReceipt, TransactionLogError>;
```

`CapturedLog` is the S1 proof bound to the exact aggregate revision/global head,
creation identity, format, active interval/raw leaf digests, seal and quarantine
state. It carries the bounded semantic/projection context needed by the caller.
It must not be fabricated from trusted legacy counters alone. Private fields or
validated constructors prevent substituting a different capture. Segmented
foreground validation visits the active source at most once, plus exact bounded
receipt/effect references; no sealed lifetime scan or synchronous rebuild.

`PreparedPublication` owns pre-encoded original event rows, optional immutable
seal, successor/current Head, `ProjectionDelta`, format/Agent update fields and
the immutable completed receipt. It records expected Agent/head/projection CAS
values. It does not expose a separately committable seal or capacity reservation.
An ordinary gate-closed legacy call selects the unchanged legacy path and cannot
emit any new-format document; an activated Agent cannot select that fallback.

The caller validates its existing domain transition, owner/lineage/retirement,
channel-history seal, runtime/checkpoint and lease holder/fence/expiry evidence.
It keeps its worktree/command/side-effect writes in the same existing
snapshot/majority transaction and passes its validated Agent predicate/fields
as `AggregateUpdate`. `persist` owns the single Agent CAS plus the planned log,
seal, head, derived-state and publication receipt. It adds exact captured-source
identity/revision/head/schema guards and format-preserving fields; callers do
not independently replace the Agent. The predicate must use S1's explicit
legacy/segmented format dispatch, not an unconditional version-1 filter. Any
failed match aborts **all** effects. A helper error must never be caught
and followed by committing the rest of the transaction.

The commit unit is therefore:

```
validated domain effects + Agent/related aggregate CAS
+ original event rows + optional seal + head CAS
+ current-state projection + indexed entity changes
+ command/checkpoint/work receipts + exact publication receipt
= one caller-owned snapshot/majority transaction
```

No runtime/native effect is performed in this transaction. Existing effect
reconciliation precedes admission where needed; recorder/checkpoint/runtime
acknowledgement occurs only after proven durable publication.

## ProjectionDelta and bootstrap

S4 reuses the historical `AgentLogProjectionState` / `AgentLogEvidenceChange`
fold and `projection.rs` envelope, extending exact per-turn model/native/usage
facts required by S2/S3. Scalar state is keyed by Agent and bound to incarnation,
evidence generation, global through and head revision. Per-turn, message,
queued-input, approval and operation evidence use indexed rows, never growing
arrays on Head. Model at turn admission is immutable even if settings change.

`ProjectionDelta` is a pure fold of <=64 proposed events against the validated
captured state. It includes expected and resulting through/head revisions,
resulting scalar state and per-entity upserts. Coalesce repeated changes to the
same entity within a batch, preserving sequential fold semantics. Sources stay
original global event references. All changes apply in `persist` before the
same commit. Unknown/stale/ahead projections fail closed. Empty-event work can
still advance aggregate/head proof and record its exact receipt.

`BootstrapPlan` is produced from S1's bounded exact legacy validation. It binds
Agent incarnation/revision/head, raw prefix proof and a complete staged evidence
generation. Preparation may stage bounded batches outside the publication
transaction under a fenced generation. No partial generation is readable. Final
publication rechecks exact source/aggregate identity and atomically installs
Agent v2 + Head + completed projection/generation pointer with the triggering
batch and receipt. A concurrent change invalidates the plan; known abort may
restart with the same operation. A 65-MiB prefix must not become a single dirty
rebuild transaction. Do not weaken the canonical Mongo cache fixture.

Lazy adoption runs only for an eligible publication. Legacy full/reserve-used
history seals as segment zero without rewriting a row or needing a spare event
slot. For S5's admitted storage-only recovery, logs may be empty: install the
successor and recovery receipt while global last_seq stays unchanged. S5 owns
failure eligibility, lifecycle mutation and runtime confirmation; S4 supplies
storage publication and exact uncertainty settlement only.

## Gate control, closure and incompatible writers

`agent_log_segments_v1` is durable, absent/disabled by default. The operator uses
revision CAS, independent of UI/task/legacy bounded-read flags. The policy keeps
explicit bounded Agent/owner selections and adds a deterministic Agent-creation
hash bucket in [0, 10,000), a stable cohort seed and basis-points threshold.
Threshold 0 selects only explicit overrides; 1,000/5,000/10,000 implement
10%/50%/100%. Selection never uses process-random hashing. Recovery policy has
its own explicit reviewed Agent cohort and does not inherit percentage selection.

Gate fencing writes occur only when admitting adoption/rollover/recovery, not
on every fitting segmented append. Adoption uses at most two durable fenced
slots across replicas; recovery admission initially uses one independent slot.
Expired/replaced slot holders cannot publish. Record slots in the gate's bounded
coordination state; immutable request admissions remain separate. Gate update
and admission touch the same durable document so closure races conflict.

Closure blocks new activation/rollover admissions. Existing segmented reads,
fitting appends, admitted cleanup and exact previously accepted publications
continue. The original request key/fingerprint/time bounds the finite rollover
exception; a turn key or lease alone is not a permit for later output. Recheck
current domain/lease/lineage even on accepted continuation. Gate/lease loss
cannot transform an unknown commit into an abort or capacity settlement.

Before any v2 emission, the release owner records the compatible reader/writer/
recovery floor and drains every incompatible foreground/background writer and
lease. The gate does not make an old executable understand a new format.
Version 2 is a cold-reader tripwire; actual old executables must be tested.
For overlap races, activation and ordinary writes share the Agent transaction
write-conflict/CAS point, including schema/revision/head/creation binding.
Every replacement preserves v2 markers. Version-1 filters reject activated rows.
Audit every direct/unconditional replacement: an unchecked stale replacement is
a rollout blocker, not something the gate silently fences. Actual drain and
old/new executable compatibility evidence remain mandatory S1/S2 release gates.

## Caller matrix and immediate registrations

| Direct insertion family | Wiring owner | Required integration |
| --- | --- | --- |
| Bare Agent append | S4 | Stable keyed attempt, atomic helper, exact single-range receipt. |
| Conversation/tool/checkpoint publication | S4 | Original command/input binding; recorder ack only after proof. |
| Worker publication including prepared claims | S4 | Existing lease/native/home/runtime effects and exact receipt. |
| Lifecycle publication | S4 | Stop/delete supersession and finite reserve rules. |
| Initial Agent admission | S4 | Created stays sequence 1; default-off creation emits legacy only. |
| Agent reuse | S4 | Original creation/command proof and current ownership. |
| Title/suggestion publication | S3 | Consume this helper and retain its durable operation identity. |
| Trigger first-message publication | S3 | Original first-input/command receipt; no redispatch. |
| Sandbox reconciliation publication | S5 | Existing domain/runtime fences; storage-only recovery uses empty batch. |

Registration request to S1: `publication`, `projection`, `writer_admission`,
`writer_bootstrap`, `writer_commit`, `writer_control`, `writer_gate`,
`writer_plan`, `writer_projection`, `writer_reconcile`, `command`; expose agreed
Head/Seal/digest/constants/captured proof to sibling writer modules. Gate and
planner unit children register in their S4-owned parents. S1 keeps its segment
registry ownership; declaration timing follows the compiling contract base.

Registration request to S2: core `agent_log_projection` module and its exports;
command evidence exports; store `agent_log_receipts`; keyed publication/shared
CapturedLog/Bootstrap ports in the assigned shared contract module; shared Mongo
test parent entries for S4 gate/publication/receipt/command tests. Add gate and
admission collections to runtime/migration authority and backup inventories,
and add actual registered tests to the canonical Mongo runner selection. S2
provides the lead-assigned heavy-build lock before the first heavy gate run.

S4 publishes small focused commits on writer-publication, one planned PR to
develop with `Part of #1350` and `Part of #1349`. Review requires success/failure,
crash/unknown-commit/retry, same-Agent rollover race, different-Agent isolation,
all capacity boundaries, gate closure, old/new compatibility and every owned
caller family. Shared S3/S5 wiring is integration acceptance, not a second
writer implementation. Review-ready never includes unregistered drafts.

## Concrete source checkpoint

The first owned implementation source is described in
`2026-11-11-segments-writer-checkpoint.md`. It supplies required policy/slot,
receipt and projection fields for other slice authors. It remains unregistered
pending the shared contract base and does not certify an executable writer.
Turn facts preserve the original nullable model, native conversation/generation,
and usage/usage-sequence references. A bounded `latest_completed` state supports
late usage without consulting mutable current settings or scanning history.
The `Turn` evidence variant boxes its facts; durable BSON remains an object.
