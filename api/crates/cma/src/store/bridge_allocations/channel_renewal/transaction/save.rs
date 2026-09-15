use super::*;
use crate::{
    sandbox::channel_renewal::model::{
        credential_id, ChannelRenewalState, RenewalClaim, RenewalPhase,
    },
    store::{
        sandbox_credentials::channel::ChannelCredentialError, sandbox_events::event_document,
        sandbox_mongo::record_bson, sandboxes::sandbox_replacement_fields,
    },
};
use layer2_core::sandbox::{SandboxEvent, SandboxEventKind};

pub(super) async fn apply(
    database: &Database,
    mut snapshot: ChannelRenewalSnapshot,
    document: &Document,
    change: &Change<'_>,
    lease: &BridgeControlPlaneLease,
    session: &mut ClientSession,
) -> Result<RenewalAdmission, MutationError> {
    let rows = majority_collection(database, "sandboxes");
    let original = snapshot.clone();
    let mut started = false;
    let now = match change {
        Change::Read | Change::Revalidate(_) => {
            super::fence(&rows, doc! {"_id":snapshot.sandbox.id.as_str(),"revision":snapshot.sandbox.revision as i64}, session).await?;
            return Ok(RenewalAdmission::Resume(snapshot));
        }
        Change::Admit { holder, now, .. } => {
            if !snapshot.due(now_ms(*now)?)? {
                return Ok(RenewalAdmission::NotDue);
            }
            if snapshot
                .renewal
                .as_ref()
                .is_some_and(|state| state.phase == RenewalPhase::MintStarted)
            {
                return Ok(RenewalAdmission::MintOutcomeUnknown);
            }
            // The scheduler's due-time observation cannot extend a human
            // authority deadline or the canonical runtime work lease.
            let admitted_at = SystemTime::now();
            claim(&mut snapshot, holder, admitted_at, lease)?;
            started = snapshot
                .renewal
                .as_ref()
                .is_some_and(|state| state.phase == RenewalPhase::MintStarted);
            admitted_at
        }
        Change::Mint {
            custody,
            material,
            now,
            ..
        } => {
            let state = snapshot.renewal.as_mut().ok_or(AllocationError::Corrupt)?;
            if state.phase != RenewalPhase::MintStarted
                || material.sandbox_id != snapshot.sandbox.id
                || material.owner != snapshot.sandbox.owner
                || material.authority.generation != state.generation
                || material.authority.provider_key_id == state.previous.provider_key_id
                || material.authority.generation
                    <= snapshot
                        .sandbox
                        .credential
                        .service_access_generation_floor()
            {
                return Err(AllocationError::IdentityMismatch.into());
            }
            custody
                .insert_for_channel(database.name(), material, session)
                .await
                .map_err(custody_error)?;
            state.replacement = Some(material.authority.clone());
            state.phase = RenewalPhase::Deliver;
            snapshot.sandbox.credential.key_generation = state.generation;
            snapshot.sandbox.credential.pending = Some(material.authority.clone());
            *now
        }
        Change::Deliver { now, .. } => {
            let state = snapshot.renewal.as_mut().ok_or(AllocationError::Corrupt)?;
            if state.phase != RenewalPhase::Deliver {
                return Err(AllocationError::InvalidTransition.into());
            }
            snapshot.sandbox.credential.current = state.replacement.clone();
            snapshot.sandbox.credential.pending = None;
            snapshot.sandbox.credential.queue_revoke(
                state.previous.provider_key_id.clone(),
                layer2_core::rfc3339_at(*now),
            );
            state.phase = RenewalPhase::Revoke;
            *now
        }
        Change::Revoke { custody, now, .. } => {
            let state = snapshot.renewal.as_mut().ok_or(AllocationError::Corrupt)?;
            if state.phase != RenewalPhase::Revoke {
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
            state.phase = RenewalPhase::Complete;
            *now
        }
    };
    let state = snapshot.renewal.as_mut().ok_or(AllocationError::Corrupt)?;
    if !matches!(change, Change::Admit { .. }) {
        state.revision = state
            .revision
            .checked_add(1)
            .ok_or(AllocationError::Corrupt)?;
        // A completed step is available to the next dispatcher immediately.
        // Keep the canonical lease until the entire replacement is complete;
        // lifecycle admission must not interleave with pending delivery.
        state.claim.expires_at_ms = (now_ms(now)? / 1000 * 1000).max(1000);
        if state.phase == RenewalPhase::Complete {
            snapshot.sandbox.work.lease = None;
        } else {
            snapshot
                .sandbox
                .work
                .lease
                .as_mut()
                .ok_or(AllocationError::Corrupt)?
                .expires_at = claim_expiry(state.claim.expires_at_ms)?;
        }
    }
    snapshot.sandbox.revision = snapshot
        .sandbox
        .revision
        .checked_add(1)
        .ok_or(AllocationError::Corrupt)?;
    snapshot.sandbox.updated_at = layer2_core::rfc3339_at(now);
    if snapshot.sandbox.updated_at < original.sandbox.updated_at {
        return Err(AllocationError::Conflict.into());
    }
    snapshot.validate()?;
    super::audit::append(database, &snapshot, change, lease, session).await?;
    let mut fields =
        sandbox_replacement_fields(&snapshot.sandbox).map_err(|_| AllocationError::Corrupt)?;
    fields.insert(
        "channel_renewal",
        record_bson(snapshot.renewal.as_ref().ok_or(AllocationError::Corrupt)?)
            .map_err(|_| AllocationError::Corrupt)?,
    );
    if matches!(change, Change::Deliver { .. }) {
        let seq = document
            .get_i64("last_seq")
            .map_err(|_| AllocationError::Corrupt)?
            .checked_add(1)
            .ok_or(AllocationError::Corrupt)?;
        fields.insert("last_seq", seq);
        let events: Collection<Document> = majority_collection(database, "sandbox_events");
        events
            .insert_one(
                event_document(&SandboxEvent {
                    sandbox_id: snapshot.sandbox.id.clone(),
                    seq: seq as u64,
                    appended_at: snapshot.sandbox.updated_at.clone(),
                    kind: SandboxEventKind::CredentialChanged {
                        key_generation: snapshot.sandbox.credential.key_generation,
                    },
                })
                .map_err(|_| AllocationError::Corrupt)?,
            )
            .session(&mut *session)
            .await?;
    }
    let changed = rows
        .update_one(
            doc! {"_id":snapshot.sandbox.id.as_str(),"revision":original.sandbox.revision as i64},
            doc! {"$set":fields},
        )
        .session(&mut *session)
        .await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    Ok(if started {
        RenewalAdmission::Started(snapshot)
    } else {
        RenewalAdmission::Resume(snapshot)
    })
}

fn claim(
    snapshot: &mut ChannelRenewalSnapshot,
    holder: &str,
    now: SystemTime,
    lease: &BridgeControlPlaneLease,
) -> Result<(), AllocationError> {
    if holder.trim().is_empty() || holder.len() > 128 {
        return Err(AllocationError::IdentityMismatch);
    }
    let now_ms = now_ms(now)?;
    if snapshot
        .renewal
        .as_ref()
        .is_some_and(|state| state.claim.expires_at_ms > now_ms)
    {
        return Err(AllocationError::LeaseUnavailable);
    }
    let remaining = lease
        .deadline()
        .saturating_duration_since(tokio::time::Instant::now())
        .min(std::time::Duration::from_secs(60));
    if remaining.is_zero() {
        return Err(AllocationError::AuthorityUnavailable);
    }
    let fence = snapshot.sandbox.work.next_fence;
    snapshot.sandbox.work.next_fence = fence.checked_add(1).ok_or(AllocationError::Corrupt)?;
    let expires_at_ms = now_ms
        .checked_add(i64::try_from(remaining.as_millis()).map_err(|_| AllocationError::Corrupt)?)
        .ok_or(AllocationError::Corrupt)?
        / 1000
        * 1000;
    if expires_at_ms <= now_ms {
        return Err(AllocationError::AuthorityUnavailable);
    }
    let claim = RenewalClaim {
        holder: holder.into(),
        fence,
        expires_at_ms,
    };
    snapshot.sandbox.work.lease = Some(layer2_core::sandbox::SandboxWorkLease {
        holder: claim.holder.clone(),
        fence,
        expires_at: claim_expiry(expires_at_ms)?,
    });
    let revision = snapshot
        .renewal
        .as_ref()
        .map_or_else(
            || {
                snapshot
                    .lifecycle_credential
                    .as_ref()
                    .map_or(Some(1), |state| state.renewal_revision.checked_add(1))
            },
            |state| state.revision.checked_add(1),
        )
        .ok_or(AllocationError::Corrupt)?;
    if snapshot
        .renewal
        .as_ref()
        .is_none_or(|state| state.phase == RenewalPhase::Complete)
    {
        let previous = snapshot
            .sandbox
            .credential
            .current
            .clone()
            .ok_or(AllocationError::Corrupt)?;
        let generation = previous
            .generation
            .checked_add(1)
            .ok_or(AllocationError::Corrupt)?;
        snapshot.renewal = Some(ChannelRenewalState {
            schema_version: 1,
            allocation_id: snapshot.allocation.id.clone(),
            revision,
            phase: RenewalPhase::MintStarted,
            generation,
            credential_id: credential_id(&snapshot.sandbox.id, generation),
            previous_credential_id: if previous.generation == 1 {
                snapshot
                    .provisioning
                    .mint_credential_id
                    .clone()
                    .ok_or(AllocationError::Corrupt)?
            } else {
                credential_id(&snapshot.sandbox.id, previous.generation)
            },
            previous,
            replacement: None,
            claim,
        });
    } else {
        let state = snapshot.renewal.as_mut().ok_or(AllocationError::Corrupt)?;
        state.claim = claim;
        state.revision = revision;
    }
    Ok(())
}

fn custody_error(error: ChannelCredentialError) -> MutationError {
    match error {
        ChannelCredentialError::Corrupt => MutationError::Core(AllocationError::Corrupt),
        ChannelCredentialError::Driver(error) => MutationError::Mongo(error),
    }
}

fn claim_expiry(milliseconds: i64) -> Result<String, AllocationError> {
    let seconds = u64::try_from(milliseconds / 1000).map_err(|_| AllocationError::Corrupt)?;
    Ok(layer2_core::rfc3339_at(
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds),
    ))
}
