use super::{MongoBridgeAllocationStore, MutationError, Resources, StageChange};
use crate::sandbox::channel_provisioning::state::{
    ChannelProvisioningPhase, ChannelProvisioningSnapshot,
};
use crate::store::{
    sandbox_events::event_document, sandbox_mongo::record_bson,
    sandboxes::sandbox_replacement_fields,
};
use layer2_core::{
    bridge::allocation::{AllocationError, AllocationState},
    sandbox::SandboxEvent,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

pub(super) async fn apply(
    store: &MongoBridgeAllocationStore,
    resources: &Resources,
    mut snapshot: ChannelProvisioningSnapshot,
    document: &Document,
    change: Option<&StageChange<'_>>,
    session: &mut ClientSession,
) -> Result<ChannelProvisioningSnapshot, MutationError> {
    let Some(change) = change else {
        if !document.contains_key("channel_provisioning") {
            let database = store.bridges.client.database(&resources.database);
            crate::store::bridge_allocations::channel_retirement::provider_finality::begin_origin(
                &database, &snapshot, session,
            )
            .await?;
        }
        let changed = resources.sandboxes.update_one(doc! {"_id":snapshot.sandbox.id.as_str(),"revision":snapshot.sandbox.revision as i64},
            doc! {"$set":{"channel_provisioning":record_bson(&snapshot.state).map_err(|_| AllocationError::Corrupt)?},"$inc":{"channel_execution_fence":1_i64}})
            .session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
        let changed = store
            .records
            .update_one(
                doc! {"_id":&snapshot.allocation.id,"revision":snapshot.allocation.revision as i64},
                doc! {"$inc":{"channel_execution_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
        return Ok(snapshot);
    };
    require_successor(&snapshot, change)?;
    let mut fields =
        sandbox_replacement_fields(&change.sandbox).map_err(|_| AllocationError::Corrupt)?;
    fields.insert(
        "channel_provisioning",
        record_bson(&change.state).map_err(|_| AllocationError::Corrupt)?,
    );
    if let Some(provenance) = &change.home_provenance {
        fields.insert(
            "channel_home_provenance",
            record_bson(provenance).map_err(|_| AllocationError::Corrupt)?,
        );
    }
    if let Some(evidence) = &change.home_prepare_evidence {
        fields.insert(
            "channel_home_prepare_evidence",
            record_bson(evidence).map_err(|_| AllocationError::Corrupt)?,
        );
    }
    if let Some(kind) = &change.event {
        let seq = document
            .get_i64("last_seq")
            .map_err(|_| AllocationError::Corrupt)?
            .checked_add(1)
            .ok_or(AllocationError::Corrupt)?;
        fields.insert("last_seq", seq);
        resources
            .events
            .insert_one(
                event_document(&SandboxEvent {
                    sandbox_id: snapshot.sandbox.id.clone(),
                    seq: seq as u64,
                    appended_at: change.sandbox.updated_at.clone(),
                    kind: kind.clone(),
                })
                .map_err(|_| AllocationError::Corrupt)?,
            )
            .session(&mut *session)
            .await?;
    }
    if let Some((custody, material)) = &change.material {
        if snapshot.state.phase != ChannelProvisioningPhase::MintStarted
            || change.state.phase != ChannelProvisioningPhase::Deliver
            || material.sandbox_id != snapshot.sandbox.id
            || material.owner != snapshot.sandbox.owner
            || change.sandbox.credential.pending.as_ref() != Some(&material.authority)
        {
            return Err(AllocationError::IdentityMismatch.into());
        }
        if let Some(intent) = &snapshot.state.mint_intent {
            intent
                .require_material(&material.authority)
                .map_err(|_| AllocationError::IdentityMismatch)?;
        }
        custody
            .insert_for_channel(&resources.database, material, session)
            .await
            .map_err(|error| match error {
                crate::store::sandbox_credentials::channel::ChannelCredentialError::Corrupt => {
                    MutationError::Core(AllocationError::Corrupt)
                }
                crate::store::sandbox_credentials::channel::ChannelCredentialError::Driver(
                    error,
                ) => MutationError::Mongo(error),
            })?;
    } else if snapshot.state.phase == ChannelProvisioningPhase::MintStarted {
        return Err(AllocationError::InvalidTransition.into());
    }
    let changed = resources
        .sandboxes
        .update_one(
            doc! {"_id":snapshot.sandbox.id.as_str(),"revision":snapshot.sandbox.revision as i64},
            doc! {"$set":fields},
        )
        .session(&mut *session)
        .await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    let mut progressed = snapshot.allocation.clone();
    if change.state.phase != ChannelProvisioningPhase::MintStarted {
        progressed.lease = None;
    }
    progressed.state = AllocationState::Provisioning;
    progressed.revision = progressed
        .revision
        .checked_add(1)
        .ok_or(AllocationError::Corrupt)?;
    progressed.updated_at_ms = super::now_ms()?;
    progressed.validate()?;
    store
        .replace(&snapshot.allocation, &progressed, session)
        .await?;
    snapshot.allocation = progressed;
    snapshot.sandbox = change.sandbox.clone();
    snapshot.state = change.state.clone();
    snapshot.home_provenance = change.home_provenance.clone();
    snapshot.home_prepare_evidence = change.home_prepare_evidence.clone();
    Ok(snapshot)
}

fn require_successor(
    snapshot: &ChannelProvisioningSnapshot,
    change: &StageChange<'_>,
) -> Result<(), AllocationError> {
    if change.previous.allocation != snapshot.allocation
        || change.previous.sandbox != snapshot.sandbox
        || change.previous.state != snapshot.state
        || change.previous.creation != snapshot.creation
        || change.previous.home_provenance != snapshot.home_provenance
        || change.previous.home_prepare_evidence != snapshot.home_prepare_evidence
        || Some(change.sandbox.revision) != snapshot.sandbox.revision.checked_add(1)
        || change.state.allocation_id != snapshot.allocation.id
        || Some(change.state.revision) != snapshot.state.revision.checked_add(1)
        || change.state.settings != snapshot.state.settings
        || (snapshot.state.mint_credential_id.is_some()
            && change.state.mint_credential_id != snapshot.state.mint_credential_id)
        || (snapshot.state.mint_intent.is_some()
            && change.state.mint_intent != snapshot.state.mint_intent)
        || change.state.phase == ChannelProvisioningPhase::Ready
        || change.sandbox.work != snapshot.sandbox.work
        || change.sandbox.updated_at < snapshot.sandbox.updated_at
    {
        return Err(AllocationError::Conflict);
    }
    snapshot.state.next(change.state.phase, &())?;
    snapshot
        .creation
        .require_creation(&snapshot.allocation, &change.sandbox)?;
    let next = ChannelProvisioningSnapshot {
        allocation: snapshot.allocation.clone(),
        sandbox: change.sandbox.clone(),
        creation: snapshot.creation.clone(),
        state: change.state.clone(),
        home_provenance: change.home_provenance.clone(),
        home_prepare_evidence: change.home_prepare_evidence.clone(),
    };
    next.validate()?;
    crate::sandbox::channel_provisioning::home_provenance::ChannelHomeProvenance::require_successor(snapshot, &next)?;
    crate::sandbox::channel_provisioning::home_prepare_evidence::ChannelHomePrepareEvidence::require_successor(snapshot, &next)?;
    Ok(())
}
