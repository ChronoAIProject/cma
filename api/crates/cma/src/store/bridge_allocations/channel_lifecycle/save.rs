use super::*;
use crate::{
    sandbox::channel_lifecycle_credentials::model::{
        ChannelLifecycleCredentialState, CredentialPhase,
    },
    store::{
        sandbox_credentials::channel::ChannelCredentialError, sandbox_mongo::record_bson,
        sandboxes::sandbox_replacement_fields,
    },
};
use layer2_core::sandbox::{SandboxEventKind, SandboxWorkStep};
use mongodb::Database;

pub(super) async fn apply(
    database: &Database,
    mut snapshot: ChannelLifecycleSnapshot,
    document: &Document,
    permit: &ChannelRevivalAuthority,
    change: &LifecycleCredentialChange<'_>,
    session: &mut ClientSession,
) -> Result<ChannelLifecycleSnapshot, MutationError> {
    if matches!(change, LifecycleCredentialChange::Publish) {
        return super::publish::apply(database, snapshot, document, permit, session).await;
    }
    if !snapshot.is_delivery() {
        return Err(AllocationError::InvalidTransition.into());
    }
    let now = SystemTime::now();
    let stamp = layer2_core::rfc3339_at(now);
    let original = snapshot.sandbox.clone();
    let mut phase_completed = false;
    match change {
        LifecycleCredentialChange::MintIntent => {
            if snapshot.current_attempt().is_some() {
                return Err(AllocationError::Conflict.into());
            }
            let previous = snapshot
                .sandbox
                .credential
                .current
                .clone()
                .ok_or(AllocationError::Corrupt)?;
            let generation = snapshot
                .sandbox
                .credential
                .next_generation()
                .ok_or(AllocationError::Corrupt)?;
            if generation
                <= snapshot
                    .sandbox
                    .credential
                    .service_access_generation_floor()
            {
                return Err(AllocationError::AuthorityUnavailable.into());
            }
            snapshot.credential = Some(ChannelLifecycleCredentialState {
                schema_version: 1,
                allocation_id: snapshot.allocation.id.clone(),
                operation: snapshot
                    .sandbox
                    .lifecycle
                    .operation
                    .as_ref()
                    .ok_or(AllocationError::Corrupt)?
                    .authority()
                    .clone(),
                runtime: snapshot
                    .sandbox
                    .runtime
                    .clone()
                    .ok_or(AllocationError::Corrupt)?,
                revision: snapshot.credential.as_ref().map_or(Ok(1), |s| {
                    s.revision.checked_add(1).ok_or(AllocationError::Corrupt)
                })?,
                renewal_revision: snapshot.renewal.as_ref().map_or_else(
                    || {
                        snapshot
                            .credential
                            .as_ref()
                            .map_or(0, |state| state.renewal_revision)
                    },
                    |state| state.revision,
                ),
                mint_fence: original
                    .work
                    .lease
                    .as_ref()
                    .ok_or(AllocationError::LeaseUnavailable)?
                    .fence,
                phase: CredentialPhase::MintStarted,
                generation,
                credential_id: snapshot.name_for(generation)?,
                previous_credential_id: snapshot.name_for(previous.generation)?,
                previous,
                replacement: None,
            });
        }
        LifecycleCredentialChange::Mint { custody, material } => {
            let state = snapshot
                .credential
                .as_mut()
                .ok_or(AllocationError::Corrupt)?;
            if state.phase != CredentialPhase::MintStarted
                || material.sandbox_id != snapshot.sandbox.id
                || material.owner != snapshot.sandbox.owner
                || material.authority.generation != state.generation
                || material.authority.provider_key_id == state.previous.provider_key_id
                || layer2_core::seconds_from_rfc3339(&material.authority.expires_at).is_none_or(
                    |expiry| {
                        expiry
                            <= now
                                .duration_since(SystemTime::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs()
                                + 300
                    },
                )
            {
                return Err(AllocationError::IdentityMismatch.into());
            }
            custody
                .insert_for_channel(database.name(), material, session)
                .await
                .map_err(custody_error)?;
            state.replacement = Some(material.authority.clone());
            state.phase = CredentialPhase::Deliver;
            snapshot.sandbox.credential.pending = Some(material.authority.clone());
            snapshot.sandbox.credential.key_generation = state.generation;
        }
        LifecycleCredentialChange::Deliver => {
            let state = snapshot
                .credential
                .as_mut()
                .ok_or(AllocationError::Corrupt)?;
            if state.phase != CredentialPhase::Deliver {
                return Err(AllocationError::InvalidTransition.into());
            }
            snapshot.sandbox.credential.current = state.replacement.clone();
            snapshot.sandbox.credential.pending = None;
            snapshot
                .sandbox
                .credential
                .queue_revoke(state.previous.provider_key_id.clone(), stamp.clone());
            state.phase = CredentialPhase::Revoke;
        }
        LifecycleCredentialChange::Revoke { custody } => {
            let state = snapshot
                .credential
                .as_mut()
                .ok_or(AllocationError::Corrupt)?;
            if state.phase != CredentialPhase::Revoke {
                return Err(AllocationError::InvalidTransition.into());
            }
            custody
                .delete_for_channel(
                    database.name(),
                    &snapshot.sandbox.id,
                    &snapshot.sandbox.owner,
                    &state.previous,
                    session,
                )
                .await
                .map_err(custody_error)?;
            snapshot.sandbox.credential.revoke_debt.clear();
            state.phase = CredentialPhase::Complete;
            let lease = original
                .work
                .lease
                .as_ref()
                .ok_or(AllocationError::LeaseUnavailable)?;
            snapshot
                .sandbox
                .complete_lifecycle_credential_delivery(&lease.holder, lease.fence, stamp.clone())
                .map_err(|_| AllocationError::InvalidTransition)?;
            phase_completed = true;
        }
        _ => return Err(AllocationError::InvalidTransition.into()),
    }
    if !matches!(change, LifecycleCredentialChange::MintIntent) {
        let state = snapshot
            .credential
            .as_mut()
            .ok_or(AllocationError::Corrupt)?;
        state.revision = state
            .revision
            .checked_add(1)
            .ok_or(AllocationError::Corrupt)?;
    }
    if !phase_completed {
        snapshot.sandbox.revision = original
            .revision
            .checked_add(1)
            .ok_or(AllocationError::Corrupt)?;
        snapshot.sandbox.updated_at = stamp.clone();
        if !matches!(change, LifecycleCredentialChange::MintIntent) {
            snapshot.sandbox.work.lease = None;
            snapshot.sandbox.work.ready_at = stamp.clone();
            snapshot.sandbox.work.attempts = 0;
        }
    }
    if snapshot.sandbox.updated_at < original.updated_at {
        return Err(AllocationError::Conflict.into());
    }
    snapshot.validate()?;
    super::audit::append(database, &snapshot, permit, change, session).await?;
    let mut fields =
        sandbox_replacement_fields(&snapshot.sandbox).map_err(|_| AllocationError::Corrupt)?;
    fields.insert(
        "channel_lifecycle_credential",
        record_bson(
            snapshot
                .credential
                .as_ref()
                .ok_or(AllocationError::Corrupt)?,
        )
        .map_err(|_| AllocationError::Corrupt)?,
    );
    if matches!(change, LifecycleCredentialChange::Deliver) {
        super::publish::events(
            database,
            document,
            &snapshot.sandbox,
            &mut fields,
            vec![SandboxEventKind::CredentialChanged {
                key_generation: snapshot.sandbox.credential.key_generation,
            }],
            session,
        )
        .await?;
    } else if phase_completed {
        super::publish::events(
            database,
            document,
            &snapshot.sandbox,
            &mut fields,
            vec![SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            }],
            session,
        )
        .await?;
    }
    let rows: Collection<Document> = majority_collection(database, "sandboxes");
    let update = rows
        .update_one(
            doc! {"_id":original.id.as_str(),"revision":original.revision as i64},
            doc! {"$set":fields},
        )
        .session(&mut *session)
        .await?;
    if update.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    Ok(snapshot)
}
fn custody_error(error: ChannelCredentialError) -> MutationError {
    match error {
        ChannelCredentialError::Driver(error) => MutationError::Mongo(error),
        _ => AllocationError::Corrupt.into(),
    }
}
