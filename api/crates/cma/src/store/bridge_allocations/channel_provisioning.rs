//! Every Channel provider stage reads and fences exact retained resources.
//! Ordinary aggregate work stays None even while the Channel worker owns a lease.
pub(super) mod transaction;
use super::{MongoBridgeAllocationStore, MutationError};
use crate::bridge::BridgeControlPlaneLease;
use crate::sandbox::channel_provisioning::state::{
    ChannelProvisioningSettings, ChannelProvisioningSnapshot, ChannelProvisioningState,
};
use crate::store::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, CommitOutcome,
};
use futures_util::TryStreamExt;
use layer2_core::bridge::allocation::{AllocationAuthority, AllocationError, AllocationRecord};
use layer2_core::sandbox::{Sandbox, SandboxEventKind};
use mongodb::bson::doc;

impl MongoBridgeAllocationStore {
    pub(crate) async fn fence_channel_mint_provisioning(
        &self,
        authority: &AllocationAuthority,
        human: &BridgeControlPlaneLease,
        snapshot: &ChannelProvisioningSnapshot,
        session: &mut mongodb::ClientSession,
    ) -> Result<(), MutationError> {
        let current = transaction::apply(
            self,
            authority,
            human,
            &snapshot.allocation,
            &snapshot.state.settings,
            None,
            session,
        )
        .await?;
        if current.sandbox != snapshot.sandbox
            || current.state != snapshot.state
            || current.creation != snapshot.creation
            || current.home_provenance != snapshot.home_provenance
            || current.home_prepare_evidence != snapshot.home_prepare_evidence
        {
            return Err(AllocationError::Conflict.into());
        }
        Ok(())
    }
    pub(crate) async fn pending_channel_allocations(
        &self,
        after: Option<&str>,
    ) -> Result<Vec<String>, AllocationError> {
        let mut filter = doc! {"$or":[{"creates_sandbox":true},{"record.placement.kind":"reuse_parent"}],"record.state":{"$in":["reserved","needs_connection","provisioning","failed_retryable"]},"_id":{"$type":"string"}};
        if let Some(after) = after {
            filter.insert("_id", doc! {"$type":"string","$gt":after});
        }
        let mut cursor = self
            .records
            .find(filter)
            .projection(doc! {"_id":1_i32})
            .sort(doc! {"_id":1_i32})
            .limit(32)
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        let mut records = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|_| AllocationError::Unavailable)?
        {
            records.push(
                document
                    .get_str("_id")
                    .map_err(|_| AllocationError::Corrupt)?
                    .to_owned(),
            );
        }
        Ok(records)
    }

    /// Discovery advances its cursor before this strict per-record decode, so
    /// one corrupt aggregate cannot hide unrelated later allocations.
    pub(crate) async fn channel_allocation(
        &self,
        id: &str,
    ) -> Result<Option<AllocationRecord>, AllocationError> {
        self.records
            .find_one(doc! {"_id":id})
            .await
            .map_err(|_| AllocationError::Unavailable)?
            .map(|document| super::records::decode_record(&document))
            .transpose()
    }

    pub(crate) async fn begin_channel_provisioning(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        allocation: &AllocationRecord,
        settings: &ChannelProvisioningSettings,
    ) -> Result<ChannelProvisioningSnapshot, AllocationError> {
        self.channel_stage_transaction(authority, lease, allocation, settings, None)
            .await
    }

    pub(crate) async fn commit_channel_provisioning(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        previous: &ChannelProvisioningSnapshot,
        sandbox: Sandbox,
        state: ChannelProvisioningState,
        event: Option<SandboxEventKind>,
    ) -> Result<ChannelProvisioningSnapshot, AllocationError> {
        self.channel_stage_transaction(
            authority,
            lease,
            &previous.allocation,
            &previous.state.settings,
            Some(StageChange {
                previous,
                sandbox,
                state,
                event,
                material: None,
                home_provenance: previous.home_provenance.clone(),
                home_prepare_evidence: previous.home_prepare_evidence.clone(),
            }),
        )
        .await
    }

    /// Secret custody and pending aggregate authority share the stage commit.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn commit_channel_minted_key(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        previous: &ChannelProvisioningSnapshot,
        sandbox: Sandbox,
        state: ChannelProvisioningState,
        custody: &crate::store::MongoSandboxCredentialStore,
        material: crate::sandbox::CredentialMaterial,
    ) -> Result<ChannelProvisioningSnapshot, AllocationError> {
        self.channel_stage_transaction(
            authority,
            lease,
            &previous.allocation,
            &previous.state.settings,
            Some(StageChange {
                previous,
                sandbox,
                state,
                event: None,
                material: Some((custody, material)),
                home_provenance: previous.home_provenance.clone(),
                home_prepare_evidence: previous.home_prepare_evidence.clone(),
            }),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn commit_channel_home_provisioning(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        previous: &ChannelProvisioningSnapshot,
        mut sandbox: Sandbox,
        phase: crate::sandbox::channel_provisioning::state::ChannelProvisioningPhase,
        home_provenance: Option<
            crate::sandbox::channel_provisioning::home_provenance::ChannelHomeProvenance,
        >,
        home_prepare_evidence: Option<
            crate::sandbox::channel_provisioning::home_prepare_evidence::ChannelHomePrepareEvidence,
        >,
    ) -> Result<ChannelProvisioningSnapshot, AllocationError> {
        sandbox.revision = previous
            .sandbox
            .revision
            .checked_add(1)
            .ok_or(AllocationError::Corrupt)?;
        sandbox.updated_at = layer2_core::now_rfc3339();
        let state = previous.state.next(
            phase,
            &(
                &sandbox.lifecycle.home,
                &home_provenance,
                &home_prepare_evidence,
            ),
        )?;
        self.channel_stage_transaction(
            authority,
            lease,
            &previous.allocation,
            &previous.state.settings,
            Some(StageChange {
                previous,
                sandbox,
                state,
                event: None,
                material: None,
                home_provenance,
                home_prepare_evidence,
            }),
        )
        .await
    }

    async fn channel_stage_transaction(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        allocation: &AllocationRecord,
        settings: &ChannelProvisioningSettings,
        change: Option<StageChange<'_>>,
    ) -> Result<ChannelProvisioningSnapshot, AllocationError> {
        for attempt in 0..16 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(
                    1_u64 << (attempt - 1).min(5),
                ))
                .await;
            }
            authority.require_record(allocation)?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            let result = match transaction::apply(
                self,
                authority,
                lease,
                allocation,
                settings,
                change.as_ref(),
                &mut session,
            )
            .await
            {
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
            if let Err(error) = authority.require_current() {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(AllocationError::Unavailable)
                }
            }
        }
        Err(AllocationError::Unavailable)
    }
}

pub(super) struct StageChange<'a> {
    pub(super) previous: &'a ChannelProvisioningSnapshot,
    pub(super) sandbox: Sandbox,
    pub(super) state: ChannelProvisioningState,
    pub(super) home_provenance:
        Option<crate::sandbox::channel_provisioning::home_provenance::ChannelHomeProvenance>,
    pub(super) home_prepare_evidence: Option<
        crate::sandbox::channel_provisioning::home_prepare_evidence::ChannelHomePrepareEvidence,
    >,
    pub(super) event: Option<SandboxEventKind>,
    pub(super) material: Option<(
        &'a crate::store::MongoSandboxCredentialStore,
        crate::sandbox::CredentialMaterial,
    )>,
}
