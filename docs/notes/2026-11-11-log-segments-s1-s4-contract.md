# Frozen S1/S4 interface for recovery and bounded consumers

Frozen 2026-11-11 18:43 SGT by the segmentation integration owner, following the
S5 interface handoff. This is a compile contract, not an executable proof claim.
S1 owns `agent_log_segments/capture.rs` and its focused children/tests; S4 owns
`writer_bootstrap.rs`, `writer_commit.rs` and their focused children/tests.
S2 owns shared root registration. These paths are added to the exclusive manifest.

Use the existing `TransactionLogError`, `AppendCapacity`, `Agent`, `Head`,
`AgentLogProjectionState`, `PublicationIdentity`, `Admission` and `Publication`.
Do not introduce another receipt schema or a second transaction framework.
`Publication` below is the existing type in `publication.rs`; the earlier
prose name `PublicationReceipt` means that type, not a new wrapper.
All signatures below are `pub(in crate::store)` unless an accessor is shown.
Opaque proof structs have private fields and no public/default/deserialization
constructor. Private implementation fields may evolve without changing callers.

## S1: captured source, owned by cma-cost

```rust
// module: crate::store::agent_log_segments::capture
use layer2_core::sandbox::{Agent, AgentLogEntry, AgentLogProjectionState};
use mongodb::{bson::Binary, ClientSession};
use super::Head;
use crate::store::{
    agent_log_integrity::TransactionLogError,
    sandbox_mongo::MongoSandboxStore,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::store) enum CaptureMode {
    Snapshot,
    Transaction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::store) enum CapturedFormat {
    Legacy,
    Segmented,
}

pub(in crate::store) struct CapturedLog { /* private verified source */ }

pub(in crate::store) async fn capture(
    store: &MongoSandboxStore,
    current: &Agent,
    session: &mut ClientSession,
    mode: CaptureMode,
) -> Result<CapturedLog, TransactionLogError>;

impl CapturedLog {
    pub fn agent(&self) -> &Agent;
    pub fn format(&self) -> CapturedFormat;
    pub fn through(&self) -> u64;
    pub fn incarnation(&self) -> &str;
    pub fn latest_entry(&self) -> Option<&AgentLogEntry>;
    pub fn projection(&self) -> &AgentLogProjectionState;
    pub fn max_opened_generation(&self) -> u64;
    pub fn max_retired_generation(&self) -> u64;

    // Exact bounded active-source proof for the writer. Legacy uses a
    // validated synthetic ordinal-zero Head; it is not persisted by capture.
    pub fn active_head(&self) -> &Head;
    pub fn active_leaf_digests(&self) -> &Binary;
}

pub(in crate::store) async fn points(
    store: &MongoSandboxStore,
    captured: &CapturedLog,
    sequences: &[u64],
    session: &mut ClientSession,
    mode: CaptureMode,
) -> Result<Vec<AgentLogEntry>, TransactionLogError>;
```

`capture` is read-only. It reads the current aggregate in the supplied session,
requires exact equality with the caller's Agent, validates creation/incarnation,
quarantine, format/head and the complete certified semantic/projection coverage.
Snapshot means an already-created read-only snapshot session; Transaction means
the caller's existing snapshot/majority transaction. Neither method starts,
commits, aborts, audits, repairs or writes quarantine. The proof records its
session/source context and is not valid in another session or after a changed
capture. Use in another mode/session fails before publication.

Legacy capture may use the existing bounded physical complete validator, up to
10,128 rows/65 MiB; it is not a new fast-path claim. It folds validated source once
and derives the exact raw-BSON leaf digests. Segmented capture uses the verified
Head, at most one active-unit validation, exact projection generation and bounded
indexed facts. It must never visit sealed lifetime history or silently run a
legacy scanner. Missing/unsupported/ahead/inconsistent semantic coverage is an
error, not an empty fact. Until that path exists a stub returns Unavailable.

`latest_entry` is exactly `through`, or None only for a proved empty source.
`projection` is certified through the same captured head, not a caller assertion.
Generation maxima cover every opened and retired native generation through the
capture; zero proves no event of that class. They are independent of the current
native generation and provider allocation generation. S4 adds these scalar maxima
to the semantic fold/envelope; S1 validates their coverage, S5 consumes them.
Absence from a point lookup alone never establishes an unpublished generation.

`points` accepts 1..=32 unique ascending positive sequences <= captured through,
returns exactly one validated original entry per requested sequence in that order,
and enforces 4 MiB total returned BSON. Missing rows, budget overflow, altered
leaves, stale context and wrong lineage fail explicitly. It reuses capture's
active proof and checks only touched sealed membership, never rescans lifetime.
An oversize requested proof is a typed unavailable/corrupt outcome as appropriate,
not permission to replay input or infer no prior work.

S5 mapping: checkpoint capture/publication use through + latest_entry; maintenance
uses latest_entry; runtime reconciliation uses projection; unpublished recovery
uses the two certified maxima; capacity admission uses exact failure/source points
plus the existing original open receipt and parent/home/work fences. The source
proof grants no user, runtime, recovery or gate authority.

## S4: atomic preparation and persistence, owned by cma-seg-lead

```rust
// module: crate::store::agent_log_segments::writer_bootstrap
pub(in crate::store) struct BootstrapPlan { /* private validated staged generation */ }

// module: crate::store::agent_log_segments::writer_commit
use layer2_core::sandbox::{Agent, AgentLogEntry};
use mongodb::{bson::{Binary, Document}, ClientSession};
use super::{
    capture::CapturedLog,
    publication::{Publication, PublicationIdentity},
    writer_admission::Admission,
    writer_bootstrap::BootstrapPlan,
};
use crate::store::{
    agent_log_integrity::{AppendCapacity, TransactionLogError},
    sandbox_mongo::MongoSandboxStore,
};

pub(in crate::store) struct PublicationInput<'a> {
    pub operation: &'a PublicationIdentity,
    pub current: &'a Agent,
    pub next: &'a Agent,
    pub logs: &'a [AgentLogEntry],
    pub capacity: AppendCapacity,
    pub result_digest: Binary,
}

pub(in crate::store) struct AggregateUpdate {
    pub filter: Document,
    pub fields: Document,
}

pub(in crate::store) struct PreparedPublication { /* private validated plan */ }

pub(in crate::store) async fn prepare(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    captured: &CapturedLog,
    input: PublicationInput<'_>,
    bootstrap: Option<&BootstrapPlan>,
    admission: Option<&Admission>,
) -> Result<PreparedPublication, TransactionLogError>;

pub(in crate::store) async fn persist(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    prepared: PreparedPublication,
    aggregate: AggregateUpdate,
) -> Result<Publication, TransactionLogError>;
```

Both calls use the caller's existing snapshot/majority transaction. `prepare`
validates the captured binding, stable key/fingerprint/time, <=64-event batch,
capacity, projection/bootstrap coverage and exact admission/gate proof. It has
no durable side effects. `PreparedPublication` binds the session, current/next
Agent, pre-encoded rows and proposed result; it cannot be constructed by callers.
`persist` owns the single Agent CAS and adds current revision/head/incarnation/
format conditions to the caller's domain/lease predicate. It writes original
rows, optional seal, successor/current Head, projection/evidence and immutable
Publication atomically with the caller's related command/work/checkpoint writes.
It neither starts nor commits nor retries the transaction. Any error requires
aborting the whole caller transaction; no catch-and-commit or ignored CAS miss.

Storage-only recovery is `logs: &[]`, original capacity-failure request identity,
`AdmissionKind::CapacityRecovery`, both independent gate proofs and a validated
legacy BootstrapPlan. The normal empty-batch exception does not grant recovery.
It seals the exact original prefix, starts successor H+1, keeps global H and
all events unchanged, and atomically records S5's non-ready durable recovery work
and exact publication. S5 writes its recovery carrier in the same session before
`persist`; a carrier failure aborts all effects. No use of ordinary first-message
work as a recovery carrier. Parent, native/home and work fences remain S5-owned.
The staged generation is built before this transaction in bounded fenced batches;
only final exact CAS makes it visible. Captured legacy proof alone is not bootstrap.

A completed Publication is storage evidence, not permission to send input or mark
Ready. Unknown commit is reconciled through S4's existing receipt reader under a
majority barrier before any new attempt. Missing receipt means Unknown; it never
means KnownAbort. A later head does not invalidate an exact completed receipt.
No implicit new key/time after restart, native resend or checkpoint acknowledgement.

## Signature-commit delivery and allocation

S1 and S4 each publish a small normal-push signature checkpoint on their OWN
assigned topic branch, then report the exact SHA on #1348/#1350 and #1351.
S1 registers `capture` in its own segment module; S4 supplies the owned writer
files and sends the child declarations to S1. If an implementation is pending,
its temporary body returns `TransactionLogError::Domain(SandboxStoreError::Unavailable(...))`
before any write. Do not use `todo!`, panic, fabricated success, `Ok(None)` fallback,
unsafe data or disabled tests. Such a checkpoint is not review-ready and cannot
activate a gate. Keep the signatures stable while replacing refusal bodies.

S5 codes against these frozen signatures now, then normally merges the exact
signature dependency heads. S2 supplies any root registrations on pinned dependency
commits without importing unrelated implementation slices. Changes to signatures
require an integration-owner amendment before callers diverge. Shared contract
checks are source formatting and compile-link tests; executable success/failure,
Mongo atomicity, no-replay, bounded work and rollback tests remain slice gates.

S5 receives the four previously unallocated pacing paths in its handoff. Its
independent flag is `CHRONO_AGENT_PARENT_NOT_READY_PACING_ENABLED=false`, consumed
only by the new parent-not-ready observation path. S2 owns config/store/worker
composition; S5 implements a builder accepting that bool in its assigned code.
Existing runtime-unavailable pacing stays as configured. No ledger behavior change.

Next checkpoint: owner signature commits before Wed 2026-11-11 19:10 SGT;
shared executable contract freeze still Wed21:00. Develop merge freeze remains
binding, and no production change is part of this handoff.
