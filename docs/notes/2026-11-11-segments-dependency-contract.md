# S1 dependency closure and transaction-scope amendment

Frozen by the segmentation integration owner, Wed 2026-11-11. This dependency checkpoint is normally mergeable and default-closed. It is not executable publication/recovery acceptance.

## Ownership decisions

| Dependency | This pinned checkpoint | Continuing owner |
|---|---|---|
| UsageFold derives and existing native identity validator export | S2 takes the two small source edits to unblock all slices | S3 retains fold/transcript implementation ownership after merging this dependency |
| MongoSandboxStore.segment_logs | S2 provides the handle and initialization; S1 lookup helper delegates to those same six handles | S2 store composition; S1 collections/registry |
| Certified opened/retired generation maxima | S2 adds required scalar fields and incremental updates to the shared projection | S4 owns subsequent projection/writer/bootstrap integration; S1 certifies capture |
| Transaction scope/stamp | S2 owns the new scope primitive; S1/S4 signatures are amended here | S1 capture implementation, S4 preparation/persistence, S5 recovery caller |

No concurrent second implementation should be written for these four small dependency changes. Normally merge this checkpoint, then continue the existing file ownership. The store handle creates no collection or validator and performs no I/O; it uses the same client/database/majority settings and names as S1. `seals` names the existing `agent_log_segments` collection, not a new collection.

## Fold/projection contracts

`UsageFold` derives Debug, Clone, Default, PartialEq, Eq, Serialize and Deserialize with `deny_unknown_fields`.

```rust
pub(super) fn validate_projection_identity(
    agent: &Agent,
    entries: &[AgentLogEntry],
) -> Result<(), String>;
```

This wrapper calls the existing native identity validator with no published ConversationOpened record; it preserves the existing unpublished-open receipt proof. It must not invent a new identity policy.

```rust
pub struct AgentLogProjectionState {
    // Existing fields remain unchanged.
    pub max_opened_generation: u64,
    pub max_retired_generation: u64,
}
```

These are required serialized fields, never serde-defaulted when reading a record. Zero means no such source event through the certified head. ConversationOpened advances the opened maximum; RuntimeGenerationRetired advances the retired maximum using its resulting runtime_generation (the same convention as S1 legacy capture). Current aggregate generation is not an absence proof. Values are checked against certified projection identity; omitted fields, impossible equal nonzero maxima, or disagreement with latest native state refuse. S4 publication and bounded bootstrap carry them atomically with the projection/head/source proof. S1 reads them only from a validated projection; it must not trust an arbitrary document solely because these fields exist.

## Exclusive transaction scope

Path: `store::agent_log_segments::transaction_scope`.

```rust
pub(in crate::store) struct TransactionScope<'session> { /* private */ }
impl<'session> TransactionScope<'session> {
    pub(in crate::store) async fn start(
        session: &'session mut ClientSession,
    ) -> Result<Self, mongodb::error::Error>;
    pub(in crate::store) fn release(self) -> &'session mut ClientSession;
    pub(in crate::store) async fn find_one(
        &mut self, collection: &Collection<Document>, filter: Document,
    ) -> Result<Option<Document>, mongodb::error::Error>;
    pub(in crate::store) async fn update_one(
        &mut self, collection: &Collection<Document>, filter: Document, update: Document,
    ) -> Result<mongodb::results::UpdateResult, mongodb::error::Error>;
    pub(in crate::store) async fn insert_one(
        &mut self, collection: &Collection<Document>, document: Document,
    ) -> Result<mongodb::results::InsertOneResult, mongodb::error::Error>;
}
```

`start` begins a fresh snapshot/majority transaction and exclusively borrows the caller session. There is no adopt/attach constructor, public raw-session accessor, mutable deref, transaction-number inference, serialization or clone. `release` consumes the scope before giving the session back for caller-controlled commit/abort/reconciliation. A retry starts a new scope with a fresh private UUID stamp even if it reuses the same session id. The private stamp binds session id and nonce. S1 stores a clone of this stamp plus database identity inside the opaque capture; both points and S4 preparation/persistence must validate it before any I/O/write.

Only audited child modules of agent_log_segments can use the internal `session`, `identity`, and `require` accessors. They must never call start/commit/abort or run transaction lifecycle commands through that internal borrow. External callers use the constrained collection methods, or release after publication. Add another constrained collection method if needed; do not expose a raw session while keeping the scope alive. This restriction is part of source review.

## Amended capture/publication signatures

Snapshot capture and snapshot points keep their existing signatures. Raw `CaptureMode::Transaction` remains a typed refusal; it cannot establish transaction identity. Transaction callers use:

```rust
pub(in crate::store) async fn capture_transaction(
    store: &MongoSandboxStore, current: &Agent,
    scope: &mut TransactionScope<'_>,
) -> Result<CapturedLog, TransactionLogError>;
pub(in crate::store) async fn points_transaction(
    store: &MongoSandboxStore, captured: &CapturedLog, sequences: &[u64],
    scope: &mut TransactionScope<'_>,
) -> Result<Vec<AgentLogEntry>, TransactionLogError>;
impl CapturedLog {
    pub(super) fn validate_transaction(
        &self, store: &MongoSandboxStore, scope: &TransactionScope<'_>,
    ) -> Result<(), TransactionLogError>;
}
```

S4 `writer_commit::{prepare,persist}` now take `scope: &mut TransactionScope<'_>` in their second parameter instead of `session: &mut ClientSession`. All other parameters/results are unchanged. S5 must use these amended signatures. S1 replaces the private snapshot-only context with a checked snapshot-or-transaction context; it cannot promote a snapshot proof into transaction authority. The new capture/points and publication bodies currently refuse before writes until the owner completes their certification. There is no successful stub.

## Gates and handoff

New tests cover independent generation maxima, missing required fields, impossible maxima and rejection of a prior stamp on a later transaction of the same session. Full Mongo tests must additionally capture on transaction A, abort/commit A, reuse that session for B and prove points/prepare/persist reject A's proof without writes. Another session, snapshot-to-transaction promotion and changed database must refuse too. Unknown commit still stays Unknown unless the unchanged receipt proof establishes Recorded.

Formatting and whitespace checks are recorded with the checkpoint. Heavy executable gates are pending available disk headroom and the shared heavy-1 lock, with two Cargo jobs and one target directory. The integration owner has no retained build outputs or disposable Mongo containers. No green integration/review-ready/deployable claim is made by this signature handoff. Develop merge freeze remains active; no production or live rule changes.
