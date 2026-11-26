//! Authority for one already published recovery operation, never new admission.
//! No principal is synthesized and no expired request authority is retained.
use super::{
    authority::{self, RecoveryAuthority},
    receipts, transactions,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use crate::store::{
    agent_log_integrity::TransactionLogError,
    agent_log_segments::transaction_scope::TransactionScope, sandbox_mongo::MongoSandboxStore,
};
use layer2_core::{
    resource_access::{ResourceAccessSnapshot, ResourceAccessStore, ResourceOwner, ResourceTarget},
    sandbox::{
        agent_capacity_recovery::{ports::RecoveryError, state::RecoveryRequest},
        store::SandboxStoreError,
    },
};

pub(super) struct AcceptedAuthority {
    snapshot: ResourceAccessSnapshot,
    request: RecoveryRequest,
}

impl RecoveryAuthority for AcceptedAuthority {
    fn snapshot(&self) -> &ResourceAccessSnapshot {
        &self.snapshot
    }
    fn require(&self) -> Result<(), TransactionLogError> {
        personal(&self.snapshot, &self.request)
    }
    fn require_request(&self, request: &RecoveryRequest) -> Result<(), TransactionLogError> {
        if request != &self.request {
            return Err(SandboxStoreError::Conflict.into());
        }
        self.require()
    }
}

fn personal(
    snapshot: &ResourceAccessSnapshot,
    request: &RecoveryRequest,
) -> Result<(), TransactionLogError> {
    let agent = snapshot
        .lineage
        .agent
        .as_ref()
        .ok_or(SandboxStoreError::Conflict)?;
    let target = ResourceTarget::Agent {
        agent_id: request.agent_id.clone(),
    };
    let scope = snapshot
        .scope(&target)
        .map_err(|_| SandboxStoreError::Conflict)?;
    if agent.creation_key != request.creation_key
        || agent.creation_fingerprint != request.creation_fingerprint
        || !matches!(scope.owner, ResourceOwner::Person { subject } if subject == agent.owner)
    {
        return Err(SandboxStoreError::Conflict.into());
    }
    Ok(())
}

/// Issue a short-lived in-process capability only after the exact durable result
/// binds the Agent, with positive current-publication proof after later progress.
/// Every call obtains a new capability; capture compares current policy, all
/// parents and shares again.
/// There is intentionally no constructor from a discovery row or owner identity.
pub(super) async fn authorize(
    store: &MongoSandboxStore,
    request: &RecoveryRequest,
) -> Result<AcceptedAuthority, RecoveryError> {
    request.validate()?;
    let snapshot = store
        .resource_access_snapshot(&ResourceTarget::Agent {
            agent_id: request.agent_id.clone(),
        })
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    personal(&snapshot, request).map_err(transactions::error)?;
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    let mut scope = TransactionScope::start(&mut session)
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    let result = async {
        let current =
            authority::capture_snapshot(store, &snapshot, &request.agent_id, &mut scope).await?;
        let agent = current.context()?.agent;
        let stored = receipts::read(store, agent, request, &mut scope)
            .await?
            .ok_or(SandboxStoreError::Conflict)?;
        receipts::require_current_result(store, agent, &stored, &mut scope).await?;
        Ok::<_, TransactionLogError>(())
    }
    .await;
    let _ = scope.release().abort_guarded().await;
    result.map_err(transactions::error)?;
    Ok(AcceptedAuthority {
        snapshot,
        request: request.clone(),
    })
}
