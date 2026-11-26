//! Frozen caller-owned publication transaction boundary.
//!
//! Preparation validates the exclusive captured transaction proof. Persistence
//! consumes only an opaque plan bound to that same scope and database, and never
//! commits or retries the caller's transaction.
use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentLogEntry};
use mongodb::bson::{Binary, Document};

use super::{
    capture::CapturedLog,
    publication::{Publication, PublicationIdentity},
    transaction_scope::{TransactionIdentity, TransactionScope},
    writer_admission::Admission,
    writer_bootstrap::BootstrapPlan,
    writer_gate::slots::Permit,
    writer_plan::{EncodedBatch, PublicationPlan, SourceFormat},
    writer_projection::ProjectionDelta,
    Head,
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
    /// Existing validated domain/lease predicate; publication adds source guards.
    pub filter: Document,
    /// `agent_replacement_fields(next)` plus caller-owned checkpoint/domain fields.
    /// Omit segment/schema/integrity fields; publication owns those and last_seq.
    pub fields: Document,
}

pub(in crate::store) struct PreparedPublication {
    transaction: TransactionIdentity,
    database: String,
    current: Agent,
    next: Agent,
    source: SourceFormat,
    before: Head,
    batch: EncodedBatch,
    plan: PublicationPlan,
    active_proof: Option<super::active_proof::ActiveProof>,
    delta: ProjectionDelta,
    evidence: Vec<planning::EvidenceWrite>,
    receipt: Publication,
    admission: Option<Admission>,
    permit: Option<Permit>,
}

#[path = "writer_existing.rs"]
mod existing;
#[path = "writer_output.rs"]
mod output;
#[path = "writer_persist.rs"]
mod persistence;
#[path = "writer_prepare.rs"]
mod planning;
pub(in crate::store) use existing::{
    publish_captured, publish_existing, publish_prepared, Recorded,
};

/// Pure preparation in the caller's existing snapshot/majority transaction.
/// Reject an unavailable or stale source proof before any database operation.
pub(in crate::store) async fn prepare(
    store: &MongoSandboxStore,
    scope: &mut TransactionScope<'_>,
    captured: &CapturedLog,
    input: PublicationInput<'_>,
    bootstrap: Option<&BootstrapPlan>,
    admission: Option<&Admission>,
) -> Result<PreparedPublication, TransactionLogError> {
    captured.validate_transaction(store, scope)?;
    super::telemetry::source(captured.active_head());
    let mut phase = super::telemetry::Phase::new("prepare", false);
    let result = Box::pin(planning::assemble(
        store, scope, captured, input, bootstrap, admission,
    ))
    .await;
    phase.finish(&result);
    result
}

/// Own the single Agent CAS and all log/projection/receipt writes in the caller's
/// transaction. The caller must abort that transaction on any helper error.
pub(in crate::store) async fn persist(
    store: &MongoSandboxStore,
    scope: &mut TransactionScope<'_>,
    prepared: PreparedPublication,
    aggregate: AggregateUpdate,
) -> Result<Publication, TransactionLogError> {
    let mut phase = super::telemetry::Phase::new("persist", prepared.plan.seal.is_some());
    let result = Box::pin(persistence::persist(store, scope, prepared, aggregate)).await;
    phase.finish(&result);
    result
}
