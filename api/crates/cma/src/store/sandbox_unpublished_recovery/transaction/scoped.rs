use super::*;
use std::collections::BTreeMap;

use crate::store::{
    agent_capacity_recovery::{lineage, source::capture_segmented_scoped},
    agent_log_segments::transaction_scope::TransactionScope,
};
use layer2_core::{sandbox::Agent, AgentId};

pub(super) struct NativeFacts(BTreeMap<AgentId, (Agent, u64, u64)>);

impl NativeFacts {
    async fn capture(
        store: &MongoSandboxStore,
        id: &SandboxId,
        scope: &mut TransactionScope<'_>,
    ) -> Result<Self, ApplyError> {
        let mut rows = BTreeMap::new();
        for agent in lineage::agents(store, id, scope).await? {
            if let Some(source) = capture_segmented_scoped(store, &agent, scope).await? {
                rows.insert(
                    agent.id.clone(),
                    (
                        agent,
                        source.max_opened_generation(),
                        source.max_retired_generation(),
                    ),
                );
            }
        }
        Ok(Self(rows))
    }

    pub(super) fn for_agent(&self, agent: &Agent) -> Result<Option<(u64, u64)>, ApplyError> {
        match self.0.get(&agent.id) {
            Some((source, opened, retired)) if source == agent => Ok(Some((*opened, *retired))),
            Some(_) => Err(ApplyError::Domain(SandboxStoreError::Conflict)),
            None => Ok(None),
        }
    }
}

/// Consume the scope before the original domain helpers use the still-open
/// transaction. No captured/native evidence is returned for reuse elsewhere.
pub(in crate::store::sandbox_unpublished_recovery) async fn load_scoped(
    store: &MongoSandboxStore,
    id: &SandboxId,
    mut scope: TransactionScope<'_>,
) -> Result<(SandboxUnpublishedRecoverySnapshot, u64), ApplyError> {
    let facts = NativeFacts::capture(store, id, &mut scope).await?;
    load_with_sources(store, id, scope.release(), Some(&facts)).await
}

pub(in crate::store::sandbox_unpublished_recovery) async fn apply_scoped(
    store: &MongoSandboxStore,
    request: &SandboxUnpublishedRecoveryRequest,
    mut scope: TransactionScope<'_>,
) -> Result<Receipt, ApplyError> {
    let facts = NativeFacts::capture(store, &request.snapshot.sandbox.id, &mut scope).await?;
    apply_with_sources(store, request, scope.release(), Some(&facts)).await
}

pub(in crate::store::sandbox_unpublished_recovery) async fn load_retained_scoped(
    store: &MongoSandboxStore,
    id: &SandboxId,
    mut scope: TransactionScope<'_>,
) -> Result<layer2_core::sandbox::RetainedRecoverySnapshot, ApplyError> {
    let facts = NativeFacts::capture(store, id, &mut scope).await?;
    load_retained_with_sources(store, id, scope.release(), Some(&facts)).await
}
