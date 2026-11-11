//! Frozen caller-owned publication transaction boundary.
//!
//! Preparation remains closed until the captured source/session proof can be
//! checked. Persistence consumes only an opaque prepared plan and never commits
//! or retries the caller's transaction.
use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentLogEntry};
use mongodb::{
    bson::{Binary, Document},
    ClientSession,
};

use super::{
    capture::CapturedLog,
    publication::{Publication, PublicationIdentity},
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
    pub filter: Document,
    pub fields: Document,
}

pub(in crate::store) struct PreparedPublication {
    session_id: Document,
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
    _store: &MongoSandboxStore,
    _session: &mut ClientSession,
    _captured: &CapturedLog,
    _input: PublicationInput<'_>,
    _bootstrap: Option<&BootstrapPlan>,
    _admission: Option<&Admission>,
) -> Result<PreparedPublication, TransactionLogError> {
    Err(pending())
}

/// Own the single Agent CAS and all log/projection/receipt writes in the caller's
/// transaction. The caller must abort that transaction on any helper error.
pub(in crate::store) async fn persist(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    prepared: PreparedPublication,
    aggregate: AggregateUpdate,
) -> Result<Publication, TransactionLogError> {
    persistence::persist(store, session, prepared, aggregate).await
}

fn pending() -> TransactionLogError {
    SandboxStoreError::Unavailable(
        "certified atomic Agent log publication is not available in this writer checkpoint".into(),
    )
    .into()
}
