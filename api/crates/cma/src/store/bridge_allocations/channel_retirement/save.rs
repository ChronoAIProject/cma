use super::*;
use crate::{
    sandbox::channel_retirement::model::{remove_key, RetirementPhase},
    store::{
        sandbox_events::event_document, sandbox_mongo::record_bson,
        sandboxes::sandbox_replacement_fields,
    },
};
use layer2_core::sandbox::{SandboxEvent, SandboxEventKind, SandboxWorkStep};

#[allow(clippy::too_many_arguments)]
pub(super) async fn apply(
    store: &MongoBridgeAllocationStore,
    permit: &ChannelRetirementAuthority,
    custody: &MongoSandboxCredentialStore,
    mut snapshot: ChannelRetirementSnapshot,
    document: &Document,
    change: &RetirementChange<'_>,
    now: SystemTime,
    session: &mut ClientSession,
) -> Result<ChannelRetirementSnapshot, MutationError> {
    let previous_revision = snapshot.sandbox.revision;
    let previous_step = snapshot.sandbox.work.step;
    if snapshot.state.is_none() && !matches!(change, RetirementChange::Begin) {
        return Err(AllocationError::InvalidTransition.into());
    }
    let database = store.bridges.client.database(&store.records.namespace().db);
    let mut state = match &snapshot.state {
        Some(state) => state.clone(),
        None => snapshot.initial_state()?,
    };
    let (action, next) = match change {
        RetirementChange::Begin
            if snapshot.state.is_none() && previous_step == SandboxWorkStep::StopAgents =>
        {
            ("stop_intent", SandboxWorkStep::StopAgents)
        }
        RetirementChange::Stopped
            if state.phase == RetirementPhase::StopIntent && !snapshot.tree.agent_live =>
        {
            state.phase = RetirementPhase::Revoke;
            (
                "agents_stopped",
                SandboxWorkStep::RevokeRetirementCredentials,
            )
        }
        RetirementChange::Revoke(key) if state.phase == RetirementPhase::Revoke => {
            let retained = state
                .keys
                .iter_mut()
                .find(|item| {
                    item.authority == key.authority && item.credential_id == key.credential_id
                })
                .ok_or(AllocationError::IdentityMismatch)?;
            if retained.revoked || key.revoked {
                return Err(AllocationError::InvalidTransition.into());
            }
            custody
                .delete_for_channel(
                    database.name(),
                    &snapshot.sandbox.id,
                    &snapshot.sandbox.owner,
                    &retained.authority,
                    session,
                )
                .await
                .map_err(custody_error)?;
            retained.revoked = true;
            remove_key(
                &mut snapshot.sandbox.credential,
                &retained.authority.provider_key_id,
            );
            let next = if state.keys.iter().all(|key| key.revoked) {
                state.phase = RetirementPhase::CredentialsCleared;
                SandboxWorkStep::TeardownRuntime
            } else {
                SandboxWorkStep::RevokeRetirementCredentials
            };
            ("key_revoked", next)
        }
        RetirementChange::RuntimeIntent if state.phase == RetirementPhase::CredentialsCleared => {
            state.phase = RetirementPhase::RuntimeIntent;
            ("runtime_intent", SandboxWorkStep::TeardownRuntime)
        }
        RetirementChange::RuntimeGone if state.phase == RetirementPhase::RuntimeIntent => {
            state.phase = RetirementPhase::RuntimeGone;
            snapshot.sandbox.runtime = None;
            ("runtime_gone", SandboxWorkStep::MarkRetired)
        }
        RetirementChange::RetainStorage
            if state.phase == RetirementPhase::RuntimeGone && snapshot.retention.is_none() =>
        {
            snapshot.retention = Some(
                crate::store::checkpoint_objects::seal_channel_retention(
                    &database,
                    &snapshot,
                    permit.human(),
                    layer2_core::rfc3339_at(now),
                    session,
                )
                .await?,
            );
            ("storage_retained", SandboxWorkStep::MarkRetired)
        }
        _ => return Err(AllocationError::InvalidTransition.into()),
    };
    let claim = snapshot
        .sandbox
        .work
        .lease
        .as_ref()
        .ok_or(AllocationError::LeaseUnavailable)?
        .clone();
    let stamp = layer2_core::rfc3339_at(now);
    snapshot
        .sandbox
        .work
        .advance(&claim.holder, claim.fence, next, stamp.clone())
        .map_err(|_| AllocationError::Conflict)?;
    snapshot.sandbox.revision = previous_revision
        .checked_add(1)
        .ok_or(AllocationError::Corrupt)?;
    if stamp < snapshot.sandbox.updated_at {
        return Err(AllocationError::Conflict.into());
    }
    snapshot.sandbox.updated_at = stamp.clone();
    snapshot.state = Some(state.clone());
    snapshot.validate()?;
    let event = if next != previous_step {
        Some(SandboxEventKind::WorkStepCompleted {
            step: previous_step,
        })
    } else if matches!(change, RetirementChange::Revoke(_)) {
        Some(SandboxEventKind::CredentialChanged {
            key_generation: snapshot.sandbox.credential.key_generation,
        })
    } else {
        None
    };
    let mut seq = document
        .get_i64("last_seq")
        .map_err(|_| AllocationError::Corrupt)?;
    let audits: Collection<Document> = majority_collection(&database, "channel_retirement_audits");
    let mut audit = doc! {"_id":format!("{}:{}:{action}",snapshot.allocation.id,snapshot.sandbox.revision),
    "document_schema_version":crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION,
    "allocation_id":&snapshot.allocation.id,"sandbox_id":snapshot.sandbox.id.as_str(),
    "sandbox_created_at":&snapshot.sandbox.created_at,"tree_fingerprint":&snapshot.tree.fingerprint,
    "operation_id":&snapshot.allocation.operation_id,"action":action,"fence":claim.fence as i64,
    "human_actor":record_bson(&permit.human().actor_context().actor).map_err(|_|AllocationError::Corrupt)?,
    "receipt":record_bson(&state).map_err(|_|AllocationError::Corrupt)?,"appended_at":&stamp};
    if let Some(retention) = &snapshot.retention {
        audit.insert(
            "storage_retention",
            record_bson(retention).map_err(|_| AllocationError::Corrupt)?,
        );
    }
    audits.insert_one(audit).session(&mut *session).await?;
    if let Some(kind) = event {
        seq = seq.checked_add(1).ok_or(AllocationError::Corrupt)?;
        let events: Collection<Document> = majority_collection(&database, "sandbox_events");
        let event = SandboxEvent {
            sandbox_id: snapshot.sandbox.id.clone(),
            seq: seq as u64,
            appended_at: stamp,
            kind,
        };
        events
            .insert_one(event_document(&event).map_err(|_| AllocationError::Corrupt)?)
            .session(&mut *session)
            .await?;
    }
    let mut fields =
        sandbox_replacement_fields(&snapshot.sandbox).map_err(|_| AllocationError::Corrupt)?;
    fields.insert("last_seq", seq);
    fields.insert(
        "channel_retirement",
        record_bson(&state).map_err(|_| AllocationError::Corrupt)?,
    );
    if let Some(retention) = &snapshot.retention {
        fields.insert(
            "channel_storage_retention",
            record_bson(retention).map_err(|_| AllocationError::Corrupt)?,
        );
    }
    let rows: Collection<Document> = majority_collection(&database, "sandboxes");
    let changed = rows
        .update_one(
            doc! {"_id":snapshot.sandbox.id.as_str(),"revision":previous_revision as i64},
            doc! {"$set":fields},
        )
        .session(&mut *session)
        .await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    Ok(snapshot)
}
