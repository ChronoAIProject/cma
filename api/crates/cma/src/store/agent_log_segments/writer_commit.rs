//! Frozen caller-owned publication transaction boundary.
//!
//! These temporary refusal bodies are the integration owner's frozen compile
//! contract. They perform no writes and cannot produce a prepared publication
//! or completed receipt. Executable planning/persistence replaces them in place.
use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentLogEntry};
use mongodb::bson::{Binary, Document};

use super::{
    capture::CapturedLog,
    publication::{Publication, PublicationIdentity},
    transaction_scope::TransactionScope,
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

pub(in crate::store) struct PreparedPublication {
    _private: (),
}

/// Pure preparation in the caller's existing snapshot/majority transaction.
/// Refuse pending source/bootstrap certification before any durable side effect.
pub(in crate::store) async fn prepare(
    _store: &MongoSandboxStore,
    _scope: &mut TransactionScope<'_>,
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
    _store: &MongoSandboxStore,
    _scope: &mut TransactionScope<'_>,
    _prepared: PreparedPublication,
    _aggregate: AggregateUpdate,
) -> Result<Publication, TransactionLogError> {
    Err(pending())
}

fn pending() -> TransactionLogError {
    SandboxStoreError::Unavailable(
        "certified atomic Agent log publication is not available in this writer checkpoint".into(),
    )
    .into()
}
