//! Retirement-only settlement of a positively identified and revoked initial mint.
use super::retirement_claim::{marker_for_state, observed, work_for_state};
use super::retirement_effect::validate_key_revocation;
use super::*;
use layer2_core::sandbox::{RuntimeBinding, SandboxStatus, SandboxWorkStep};

fn eligible_parent(sandbox: &Sandbox) -> bool {
    let eligible_step = match sandbox.status {
        SandboxStatus::Retiring => matches!(
            sandbox.work.step,
            SandboxWorkStep::StopAgents
                | SandboxWorkStep::RevokeRetirementCredentials
                | SandboxWorkStep::TeardownRuntime
                | SandboxWorkStep::MarkRetired
        ),
        SandboxStatus::Retired => matches!(
            sandbox.work.step,
            SandboxWorkStep::RetryRetiredRevocations
                | SandboxWorkStep::WaitForPurge
                | SandboxWorkStep::Purge
        ),
        _ => false,
    };
    eligible_step
        && sandbox.credential.source.is_managed()
        && sandbox.credential.key_generation == 0
        && !sandbox.credential.delivered
        && sandbox.credential.current.is_none()
        && sandbox.credential.pending.is_none()
        && sandbox.credential.revoke_debt.is_empty()
        && sandbox.lifecycle.suspended_runtime.is_none()
}

fn native(runtime: &RuntimeBinding) -> bool {
    runtime
        .runtime_id
        .strip_prefix("agent-sandbox:")
        .is_some_and(|id| !id.is_empty())
}

fn parent_identity(sandbox: &Sandbox) -> Result<Document, CredentialCustodyError> {
    let home = sandbox.lifecycle.home.as_ref().map(|home| &home.request);
    Ok(doc! {
        "sandbox_id":sandbox.id.as_str(), "owner":sandbox.owner.as_str(),
        "created_at":&sandbox.created_at,
        "runtime_generation_high_water":mongodb::bson::serialize_to_bson(&sandbox.lifecycle.runtime_generation_high_water).map_err(|_| corrupt())?,
        "home":mongodb::bson::serialize_to_bson(&home).map_err(|_| corrupt())?,
    })
}

fn receipt_evidence(row: &Document) -> Result<Document, CredentialCustodyError> {
    let (intent, _, _) = work_for_state(row, "key_revoked")?;
    let marker = marker_for_state(row, "key_revoked")?;
    validate_key_revocation(marker, &intent)?;
    Ok(doc! {
        "issuance":marker.get_document("issuance").map_err(|_| corrupt())?,
        "owner_authority":marker.get_document("owner_authority").map_err(|_| corrupt())?,
        "late_provider_authority":marker.get_document("late_provider_authority").map_err(|_| corrupt())?,
        "revocation_observation":marker.get_document("revocation_observation").map_err(|_| corrupt())?,
    })
}

/// An existing receipt can be consumed by a later lifecycle claim after the
/// runtime is gone. The parent incarnation, generation and home never change.
pub(in crate::store::sandbox_credentials) fn settled_for(
    row: &Document,
    sandbox: &Sandbox,
) -> Result<bool, CredentialCustodyError> {
    if !eligible_parent(sandbox) {
        return Ok(false);
    }
    let intent = decode(row.clone())?;
    if intent.owner != sandbox.owner || intent.sandbox_id != sandbox.id {
        return Err(corrupt());
    }
    let Some(marker) = row.get("retirement_custody") else {
        return Ok(false);
    };
    let marker = marker.as_document().ok_or_else(corrupt)?;
    let Some(receipt) = marker.get("settlement") else {
        return Ok(false);
    };
    let receipt = receipt.as_document().ok_or_else(corrupt)?;
    let runtime: RuntimeBinding =
        mongodb::bson::deserialize_from_bson(receipt.get("runtime").cloned().ok_or_else(corrupt)?)
            .map_err(|_| corrupt())?;
    let claim = receipt.get_document("claim").map_err(|_| corrupt())?;
    if receipt.get_i64("version").ok() != Some(1)
        || receipt.get_document("parent").ok() != Some(&parent_identity(sandbox)?)
        || receipt.get_document("evidence").ok() != Some(&receipt_evidence(row)?)
        || !native(&runtime)
        || sandbox
            .runtime
            .as_ref()
            .is_some_and(|current| current != &runtime)
        || receipt
            .get_i64("parent_revision")
            .ok()
            .is_none_or(|revision| revision <= 0)
        || claim.get_str("holder").ok().is_none_or(str::is_empty)
        || claim.get_i64("fence").ok().is_none_or(|fence| fence <= 0)
        || receipt.get_datetime("recorded_at").is_err()
    {
        return Err(corrupt());
    }
    Ok(true)
}

pub(in crate::store::sandbox_credentials) async fn settle(
    store: &MongoSandboxCredentialStore,
    sandbox: &Sandbox,
    original: &CredentialIssuance,
) -> Result<bool, CredentialCustodyError> {
    if !eligible_parent(sandbox) {
        return Ok(false);
    }
    // Fence the actual lifecycle claim and ensure the receipt's provenance is
    // taken from the current aggregate, not an in-memory cleanup projection.
    let mut session = transactions::start(store, sandbox).await?;
    let outcome = async {
        let aggregate = store
            .sandboxes
            .find_one(doc! {"_id":sandbox.id.as_str()})
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .ok_or_else(corrupt)?;
        let parent = crate::store::sandboxes::read_sandbox(&aggregate).map_err(|_| corrupt())?;
        let row = store
            .intents
            .find_one(doc! {"_id":sandbox.id.as_str()})
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .ok_or_else(corrupt)?;
        let intent = decode(row.clone())?;
        let mut requested = original.clone();
        requested.next_check_at = intent.next_check_at.clone();
        if intent != requested
            || intent.owner != parent.owner
            || intent.sandbox_id != parent.id
            || !eligible_parent(&parent)
        {
            return Ok(false);
        }
        let filter = doc! {"sandbox_id":parent.id.as_str()};
        if store
            .collection
            .find_one(filter.clone())
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .is_some()
            || store
                .revocations
                .find_one(filter)
                .session(&mut session)
                .await
                .map_err(unavailable)?
                .is_some()
        {
            return Ok(false);
        }
        if settled_for(&row, &parent)? {
            return settled_for(&row, sandbox);
        }
        if !crate::sandbox::initial_retirement_issuance_candidate(&parent)
            || parent_identity(&parent)? != parent_identity(sandbox)?
            || parent.runtime != sandbox.runtime
        {
            return Ok(false);
        }
        let Some(runtime) = parent.runtime.as_ref().filter(|runtime| native(runtime)) else {
            return Ok(false);
        };
        let Some(marker) = row.get("retirement_custody") else {
            return Ok(false);
        };
        let marker = marker.as_document().ok_or_else(corrupt)?;
        if marker.get_str("state").ok() != Some("key_revoked") {
            return Ok(false);
        }
        let evidence = receipt_evidence(&row)?;
        let parent_identity = parent_identity(&parent)?;
        let lease = parent.work.lease.as_ref().ok_or_else(corrupt)?;
        let claim = doc! {"holder":&lease.holder, "fence":i64::try_from(lease.fence).map_err(|_| corrupt())?};
        let receipt = doc! {"version":1_i64, "parent":{"$literal":parent_identity},
        "runtime":{"$literal":mongodb::bson::serialize_to_bson(runtime).map_err(|_| corrupt())?},
        "parent_revision": i64::try_from(parent.revision).map_err(|_| corrupt())?,
        "claim":{"$literal":claim},
        "evidence":{"$literal":evidence}, "recorded_at":"$$NOW"};
        let updated = store
            .intents
            .update_one(
                observed(&row)?,
                vec![doc! {"$set":{"retirement_custody.settlement":receipt}}],
            )
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        Ok(updated.matched_count == 1)
    }
    .await;
    transactions::finish(&mut session, outcome).await
}
