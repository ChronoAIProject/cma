//! Frozen caller-owned publication transaction boundary.
//!
//! Preparation remains closed until the captured source/session proof can be
//! checked. Persistence consumes only an opaque prepared plan and never commits
//! or retries the caller's transaction.
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
    delta: ProjectionDelta,
    evidence: Vec<planning::EvidenceWrite>,
    receipt: Publication,
    admission: Option<Admission>,
    permit: Option<Permit>,
}

#[path = "writer_persist.rs"]
mod persistence;
#[path = "writer_prepare.rs"]
mod planning;

/// Pure preparation in the caller's existing snapshot/majority transaction.
/// Refuse pending source/bootstrap certification before any durable side effect.
pub(in crate::store) async fn prepare(
    store: &MongoSandboxStore,
    scope: &mut TransactionScope<'_>,
    captured: &CapturedLog,
    input: PublicationInput<'_>,
    bootstrap: Option<&BootstrapPlan>,
    admission: Option<&Admission>,
) -> Result<PreparedPublication, TransactionLogError> {
    captured.validate_transaction(store, scope)?;
    planning::assemble(store, scope, captured, input, bootstrap, admission).await
}

/// Own the single Agent CAS and all log/projection/receipt writes in the caller's
/// transaction. The caller must abort that transaction on any helper error.
pub(in crate::store) async fn persist(
    store: &MongoSandboxStore,
    scope: &mut TransactionScope<'_>,
    prepared: PreparedPublication,
    aggregate: AggregateUpdate,
) -> Result<Publication, TransactionLogError> {
    persistence::persist(store, scope, prepared, aggregate).await
}
