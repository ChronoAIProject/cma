//! Shared-parent writes use the same parent and resource-policy serialization
//! as ordinary creation. No Channel root admission marker is ever written.
use super::*;
use layer2_core::bridge::allocation::{AllocationPlacement, AllocationReuseProof};

mod capacity;
mod children;
mod lifecycle;
mod startup;
mod transaction;
use crate::bridge::BridgeControlPlaneLease;
pub(crate) use crate::sandbox::channel_reuse::state::*;
use crate::sandbox::{
    channel_children::ChannelChildEvidence,
    channel_provisioning::state::ChannelProvisioningSettings,
};

impl MongoBridgeAllocationStore {
    pub(super) async fn fence_reused_parent(
        &self,
        authority: &AllocationAuthority,
        proof: &AllocationReuseProof,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let resources = &self
            .invokes
            .as_ref()
            .ok_or(AllocationError::AuthorityUnavailable)?
            .resources;
        let config = authority.config()?;
        let target = config
            .placement
            .reused_parent()
            .ok_or(AllocationError::IncompatibleTarget)?;
        let current =
            crate::store::resource_access::read_execution_snapshot(resources, &target, session)
                .await
                .map_err(|error| match error {
                    crate::store::resource_access::ReadError::Driver(error) => {
                        MutationError::Mongo(error)
                    }
                    _ => MutationError::Core(AllocationError::IncompatibleTarget),
                })?;
        if &current != proof.resources() {
            return Err(AllocationError::Conflict.into());
        }
        proof.parent().require(config, proof.profiles(), &current)?;
        resources
            .ordinary_reuse_parent(&current.lineage.sandbox, session)
            .await
            .map_err(|error| match error {
                crate::store::bridge_reuse_parent::ParentError::Core(error) => {
                    MutationError::Core(error)
                }
                crate::store::bridge_reuse_parent::ParentError::Driver(error) => {
                    MutationError::Mongo(error)
                }
            })?;
        crate::store::resource_access::fence_lineage(resources, &current, session)
            .await
            .map_err(|error| match error {
                crate::store::resource_access::ReadError::Driver(error) => {
                    MutationError::Mongo(error)
                }
                _ => MutationError::Core(AllocationError::Conflict),
            })?;
        if let Some(policy) = &current.policy {
            let changed = resources.resource_policies.update_one(
                doc! {"_id": current.lineage.sandbox.id.as_str(), "revision": policy.revision as i64,
                    "document_schema_version": crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION},
                doc! {"$inc": {"channel_reuse_fence": 1_i64}}).session(&mut *session).await?;
            if changed.matched_count != 1 {
                return Err(AllocationError::Conflict.into());
            }
        }
        // A first policy/share insertion also fences this same root through
        // resource_access::mutation::fence_lineage, including the absent-policy case.
        Ok(())
    }
}

impl MongoBridgeAllocationStore {
    pub(super) async fn fence_reuse_cleanup(
        &self,
        authority: &AllocationAuthority,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let AllocationAuthority::Reuse(proof) = authority else {
            return Ok(());
        };
        let resources = &self
            .invokes
            .as_ref()
            .ok_or(AllocationError::AuthorityUnavailable)?
            .resources;
        let current = proof.resources();
        let guarded = resources.sandboxes.update_one(
            doc! {"_id":current.lineage.sandbox.id.as_str(),"channel_reuse_cleanup_execution":{"$exists":false}},
            doc! {"$inc":{"owned_workspace_admission_fence":1_i64}},
        ).session(&mut *session).await?;
        if guarded.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
        if let Some(workspace) = &current.lineage.workspace {
            let guarded = resources.workspaces.update_one(
                doc! {"_id":workspace.id.as_str(),"revision":workspace.revision as i64,
                    "channel_reuse_cleanup_execution":{"$exists":false},"owned_workspace_cleanup_admission_v1":{"$exists":false}},
                doc! {"$inc":{"owned_workspace_admission_fence":1_i64}},
            ).session(&mut *session).await?;
            if guarded.matched_count != 1 {
                return Err(AllocationError::Conflict.into());
            }
        }

        Ok(())
    }

    pub(crate) async fn begin_channel_reuse(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        allocation: &AllocationRecord,
        settings: &ChannelProvisioningSettings,
    ) -> Result<ChannelReuseSnapshot, AllocationError> {
        self.reuse_transaction(authority, lease, allocation, settings, None)
            .await
    }
    pub(crate) async fn commit_reuse_child(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: &ChannelReuseSnapshot,
        evidence: &ChannelChildEvidence,
    ) -> Result<ChannelReuseSnapshot, AllocationError> {
        self.reuse_transaction(
            authority,
            lease,
            &snapshot.allocation,
            &snapshot.state.settings,
            Some((snapshot, evidence)),
        )
        .await
    }
    async fn reuse_transaction(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        allocation: &AllocationRecord,
        settings: &ChannelProvisioningSettings,
        change: Option<(&ChannelReuseSnapshot, &ChannelChildEvidence)>,
    ) -> Result<ChannelReuseSnapshot, AllocationError> {
        for attempt in 0..16 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(
                    1_u64 << (attempt - 1).min(5),
                ))
                .await;
            }
            transaction::require_current(authority, lease, allocation)?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            let result = async {
                let current =
                    transaction::load(self, authority, lease, allocation, settings, &mut session)
                        .await?;
                if let Some((previous, evidence)) = change {
                    children::apply(self, authority, &current, previous, evidence, &mut session)
                        .await
                } else {
                    Ok(current)
                }
            }
            .await;
            let result = match result {
                Ok(result) => result,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Core(error) => return Err(error),
                        MutationError::Mongo(error) if is_transient(&error) => continue,
                        MutationError::Mongo(_) => return Err(AllocationError::Unavailable),
                    }
                }
            };
            if let Err(error) = transaction::require_current(authority, lease, allocation) {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(AllocationError::Unavailable);
                }
            }
        }
        Err(AllocationError::Unavailable)
    }
}
