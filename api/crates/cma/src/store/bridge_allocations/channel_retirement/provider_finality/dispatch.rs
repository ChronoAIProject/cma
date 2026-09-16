use super::*;
use crate::{
    bridge::BridgeControlPlaneLease,
    sandbox::channel_provisioning::state::ChannelProvisioningSnapshot,
    store::{
        sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
        MongoBridgeAllocationStore,
    },
};
use layer2_core::bridge::allocation::AllocationAuthority;

pub(crate) struct RuntimeDispatchToken {
    pub operation_id: String,
}

pub(crate) async fn begin_origin(
    database: &Database,
    snapshot: &ChannelProvisioningSnapshot,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    // Called only in the transaction that first creates channel_provisioning.
    // Missing journals on already-started provisioning are never backfilled.
    if snapshot.state.revision != 1
        || snapshot.sandbox.runtime.is_some()
        || snapshot.sandbox.lifecycle.home.is_some()
    {
        return Err(AllocationError::IdentityMismatch.into());
    }
    let journal = DispatchJournal {
        origin: ProviderDispatchOrigin::capture(snapshot)?,
        runtime: Vec::new(),
    };
    majority_collection::<Document>(database, "channel_provider_dispatch_journals")
        .insert_one(
            doc! {"_id":&snapshot.allocation.id,"record":record_bson(&journal)
            .map_err(|_| AllocationError::Corrupt)?},
        )
        .session(session)
        .await?;
    Ok(())
}

enum Change<'a> {
    Begin {
        operation_id: &'a str,
        request: &'a layer3_adapter::RuntimeProvisionRequest,
    },
    Complete {
        token: &'a RuntimeDispatchToken,
        receipt: &'a RuntimeProvisionReceipt,
    },
}

impl MongoBridgeAllocationStore {
    pub(crate) async fn begin_channel_runtime_dispatch(
        &self,
        authority: &AllocationAuthority,
        human: &BridgeControlPlaneLease,
        snapshot: &ChannelProvisioningSnapshot,
        request: &layer3_adapter::RuntimeProvisionRequest,
    ) -> Result<Option<RuntimeDispatchToken>, AllocationError> {
        let operation_id = format!(
            "{}:runtime:{}",
            snapshot.allocation.operation_id,
            uuid::Uuid::new_v4()
        );
        let recorded = self
            .change_runtime_dispatch(
                authority,
                human,
                snapshot,
                Change::Begin {
                    operation_id: &operation_id,
                    request,
                },
            )
            .await?;
        Ok(recorded.then_some(RuntimeDispatchToken { operation_id }))
    }

    pub(crate) async fn complete_channel_runtime_dispatch(
        &self,
        authority: &AllocationAuthority,
        human: &BridgeControlPlaneLease,
        snapshot: &ChannelProvisioningSnapshot,
        token: &RuntimeDispatchToken,
        receipt: &RuntimeProvisionReceipt,
    ) -> Result<(), AllocationError> {
        if !self
            .change_runtime_dispatch(
                authority,
                human,
                snapshot,
                Change::Complete { token, receipt },
            )
            .await?
        {
            return Err(AllocationError::Conflict);
        }
        Ok(())
    }

    async fn change_runtime_dispatch(
        &self,
        authority: &AllocationAuthority,
        human: &BridgeControlPlaneLease,
        snapshot: &ChannelProvisioningSnapshot,
        change: Change<'_>,
    ) -> Result<bool, AllocationError> {
        let database = self.bridges.client.database(&self.records.namespace().db);
        for _ in 0..16 {
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            let result: Result<bool, MutationError> = async {
                self.fence_channel_mint_provisioning(authority, human, snapshot, &mut session).await?;
                let Some(mut journal) = read_journal(&database, &snapshot.allocation.id, &mut session).await? else { return Ok(false); };
                if journal.origin != ProviderDispatchOrigin::capture(snapshot)? {
                    return Err(AllocationError::IdentityMismatch.into());
                }
                if journal.origin.original_home_policy.is_some() { return Ok(false); }
                if snapshot.state.phase != crate::sandbox::channel_provisioning::state::ChannelProvisioningPhase::Runtime
                    || snapshot.sandbox.runtime.is_some() || snapshot.sandbox.lifecycle.home.is_some()
                { return Err(AllocationError::InvalidTransition.into()); }
                match &change {
                    Change::Begin { operation_id, request } => {
                        let expected = crate::sandbox::channel_retirement::provider_finality::provisioning_request(snapshot);
                        if **request != expected || journal.runtime.len() >= 64 { return Err(AllocationError::IdentityMismatch.into()); }
                        journal.runtime.push(RuntimeDispatch { operation_id: (*operation_id).to_owned(),
                            request_sha256: layer3_adapter::provider_request_digest(request).map_err(|_| AllocationError::Corrupt)?,
                            generation: 1, receipt: None });
                    }
                    Change::Complete { token, receipt } => {
                        receipt.validate().map_err(|_| AllocationError::IdentityMismatch)?;
                        let row = journal.runtime.iter_mut().find(|row| row.operation_id == token.operation_id)
                            .ok_or(AllocationError::IdentityMismatch)?;
                        if row.receipt.is_some() || receipt.operation_id() != row.operation_id
                            || receipt.request_sha256() != row.request_sha256 { return Err(AllocationError::IdentityMismatch.into()); }
                        row.receipt = Some((*receipt).clone());
                    }
                }
                let changed = majority_collection::<Document>(&database, "channel_provider_dispatch_journals")
                    .update_one(doc! {"_id":&snapshot.allocation.id}, doc! {"$set":{"record":record_bson(&journal)
                        .map_err(|_| AllocationError::Corrupt)?}}).session(&mut session).await?;
                if changed.matched_count != 1 { return Err(AllocationError::Conflict.into()); }
                Ok(true)
            }.await;
            let changed = match result {
                Ok(changed) => changed,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Core(error) => return Err(error),
                        MutationError::Mongo(error)
                            if crate::store::sandbox_mongo::is_transient(&error) =>
                        {
                            continue;
                        }
                        _ => return Err(AllocationError::Unavailable),
                    }
                }
            };
            if authority.require_current().is_err() {
                let _ = session.abort_transaction().await;
                return Err(AllocationError::AuthorityUnavailable);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(changed),
                CommitOutcome::Retry(_) => continue,
                _ => return Err(AllocationError::Unavailable),
            }
        }
        Err(AllocationError::Unavailable)
    }
}
