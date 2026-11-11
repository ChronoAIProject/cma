//! Authorized bounded status; a work row alone never proves storage completion.
use layer2_core::resource_access::AuthorizedResource;
use layer2_core::sandbox::agent_capacity_recovery::{
    state::{RecoveryPhase, RecoveryRequest},
    CapacityRecoveryDisposition,
};
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::AgentId;
use serde::Serialize;

use super::{authority, records};
use crate::store::{
    agent_log_integrity::TransactionLogError,
    agent_log_segments::transaction_scope::TransactionScope, sandbox_mongo::MongoSandboxStore,
};

#[derive(Debug, Serialize)]
pub(crate) struct RecoveryStatus {
    pub agent_id: AgentId,
    pub disposition: CapacityRecoveryDisposition,
    pub phase: Option<RecoveryPhase>,
    pub attempts: u32,
    pub retry_at: Option<String>,
    /// The existing request identity is not an authorization or success token.
    pub operation_key: Option<String>,
}

/// Call in an existing read transaction; no gate, carrier, audit or log writes.
/// The supplied authorization must be freshly obtained by the existing resource
/// service. The transaction rechecks its exact policy and ancestry before release.
pub(in crate::store) async fn read(
    store: &MongoSandboxStore,
    authorized: &AuthorizedResource,
    id: &AgentId,
    request: Option<&RecoveryRequest>,
    now: &str,
    scope: &mut TransactionScope<'_>,
) -> Result<RecoveryStatus, TransactionLogError> {
    let current = authority::capture(store, authorized, id, scope).await?;
    let context = current.context()?;
    let mut status = RecoveryStatus {
        agent_id: id.clone(),
        disposition: context.classify(now),
        phase: None,
        attempts: 0,
        retry_at: None,
        operation_key: None,
    };
    if let Some(request) = request {
        request
            .validate()
            .map_err(|_| SandboxStoreError::Conflict)?;
        if request.agent_id != *id
            || request.creation_key != context.agent.creation_key
            || request.creation_fingerprint != context.agent.creation_fingerprint
        {
            return Err(SandboxStoreError::Conflict.into());
        }
        let key = request.key().map_err(|_| SandboxStoreError::Conflict)?;
        if let Some(work) = records::read(store, &key, scope).await? {
            if work.request != *request {
                return Err(SandboxStoreError::Conflict.into());
            }
            // This checkpoint has no completed-publication reader for this
            // carrier. Refuse successful status until exact receipt integration
            // lands; a deserializable row alone never establishes completion.
            if work.phase == RecoveryPhase::Completed {
                return Err(SandboxStoreError::Unavailable(
                    "exact capacity recovery completion receipt requires reconciliation".into(),
                )
                .into());
            }
            status.phase = Some(work.phase);
            status.attempts = work.attempts;
            status.retry_at =
                matches!(work.phase, RecoveryPhase::Admitted).then_some(work.ready_at);
            status.operation_key = Some(key);
        }
        // An absent row exposes no phase; it is not a KnownAbort assertion.
    }
    authority::require(authorized)?;
    Ok(status)
}
