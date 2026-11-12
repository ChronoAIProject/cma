//! Authorized bounded status; a work row alone never proves storage completion.
use layer2_core::resource_access::AuthorizedResource;
use layer2_core::sandbox::agent_capacity_recovery::{
    commands::RecoveryStatus,
    state::{RecoveryPhase, RecoveryRequest},
};
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::AgentId;

use super::{authority, receipts};
use crate::store::{
    agent_log_integrity::TransactionLogError,
    agent_log_segments::transaction_scope::TransactionScope, sandbox_mongo::MongoSandboxStore,
};

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
        if let Some(stored) = receipts::read(store, context.agent, request, scope).await? {
            let work = stored.work;
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
