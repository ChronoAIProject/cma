//! Canonical Retired and the exact retain-only proof commit as one transaction.
use super::*;
use crate::store::{channel_history::HistorySeal, sandbox_mongo::record_bson};
use layer2_core::sandbox::{
    Sandbox, SandboxEvent, SandboxEventKind, SandboxStatus, SandboxWorkStep,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FinalizationReceipt {
    pub schema_version: u16,
    pub allocation_id: String,
    pub allocation_operation_id: String,
    pub sandbox_sha256: String,
    pub provider_finality_sha256: String,
    pub retirement_state_sha256: String,
    pub storage_audit_id: String,
    pub storage_audit_sha256: String,
    pub history: HistorySeal,
    pub retired_at: String,
}
impl FinalizationReceipt {
    fn digest(&self) -> Result<String, AllocationError> {
        digest(self)
    }
    fn require(&self, sandbox: &Sandbox) -> Result<(), AllocationError> {
        if self.schema_version != 1
            || self.allocation_id != self.history.policy.allocation_id
            || self.history.sandbox_id != sandbox.id.as_str()
            || self.history.sandbox_created_at != sandbox.created_at
            || self.sandbox_sha256 != digest(sandbox)?
            || sandbox.status != SandboxStatus::Retired
            || sandbox.retired_at.as_ref() != Some(&self.retired_at)
            || self.retired_at != self.history.sealed_at
            || sandbox.purge_after.is_some()
            || sandbox.runtime.is_some()
            || sandbox.lifecycle.home.is_some()
            || sandbox.work.step != SandboxWorkStep::None
            || sandbox.work.lease.is_some()
            || sandbox.credential.current.is_some()
            || sandbox.credential.pending.is_some()
            || !sandbox.credential.revoke_debt.is_empty()
            || [
                &self.provider_finality_sha256,
                &self.retirement_state_sha256,
                &self.storage_audit_sha256,
            ]
            .iter()
            .any(|value| {
                value.len() != 64
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
        {
            return Err(AllocationError::IdentityMismatch);
        }
        Ok(())
    }
}

pub(super) async fn apply(
    store: &MongoBridgeAllocationStore,
    permit: &ChannelRetirementAuthority,
    mut snapshot: ChannelRetirementSnapshot,
    document: &Document,
    now: SystemTime,
    session: &mut ClientSession,
) -> Result<ChannelRetirementSnapshot, MutationError> {
    snapshot.validate()?;
    if snapshot.sandbox.work.step != SandboxWorkStep::MarkRetired
        || snapshot.retention.is_none()
        || snapshot.retention_audit.is_none()
    {
        return Err(AllocationError::InvalidTransition.into());
    }
    let database = store.bridges.client.database(&store.records.namespace().db);
    let Some(provider) =
        super::provider_finality::read_verified(&database, &snapshot, session).await?
    else {
        return Ok(snapshot);
    };
    provider.require(&snapshot)?;
    // Only the original immutable NotConfigured path is supported here.
    if snapshot.provisioning.settings.home_policy.is_some()
        || snapshot.sandbox.lifecycle.home.is_some()
    {
        return Ok(snapshot);
    }
    let stamp = layer2_core::rfc3339_at(now);
    if stamp < snapshot.sandbox.updated_at {
        return Err(AllocationError::Conflict.into());
    }
    let history =
        match crate::store::channel_history::seal(&database, &snapshot, &stamp, session).await? {
            Ok(history) => history,
            Err(hold) => {
                snapshot.history.hold = Some(hold);
                return Ok(snapshot);
            }
        };
    let previous = snapshot.sandbox.revision;
    let claim = snapshot
        .sandbox
        .work
        .lease
        .clone()
        .ok_or(AllocationError::LeaseUnavailable)?;
    snapshot
        .sandbox
        .work
        .advance(
            &claim.holder,
            claim.fence,
            SandboxWorkStep::None,
            stamp.clone(),
        )
        .map_err(|_| AllocationError::Conflict)?;
    snapshot
        .sandbox
        .transition(SandboxStatus::Retired, stamp.clone(), None)
        .map_err(|_| AllocationError::InvalidTransition)?;
    snapshot.sandbox.purge_after = None;
    snapshot
        .sandbox
        .validate()
        .map_err(|_| AllocationError::Corrupt)?;
    let audit = snapshot
        .retention_audit
        .as_ref()
        .ok_or(AllocationError::Corrupt)?;
    let receipt = FinalizationReceipt {
        schema_version: 1,
        allocation_id: snapshot.allocation.id.clone(),
        allocation_operation_id: snapshot.allocation.operation_id.clone(),
        sandbox_sha256: digest(&snapshot.sandbox)?,
        provider_finality_sha256: provider.digest()?,
        retirement_state_sha256: digest(&snapshot.state)?,
        storage_audit_id: audit.id.clone(),
        storage_audit_sha256: audit.sha256.clone(),
        history,
        retired_at: stamp.clone(),
    };
    receipt.require(&snapshot.sandbox)?;
    let receipt_bson = record_bson(&receipt).map_err(|_| AllocationError::Corrupt)?;
    let receipt_sha256 = receipt.digest()?;
    majority_collection::<Document>(&database, "channel_retirement_finalizations").insert_one(doc! {
        "_id": &snapshot.allocation.id, "document_schema_version": crate::store::schema::DOCUMENT_SCHEMA_VERSION,
        "sandbox_id": snapshot.sandbox.id.as_str(), "record": receipt_bson.clone(), "sha256": &receipt_sha256,
    }).session(&mut *session).await?;
    let seq = document
        .get_i64("last_seq")
        .map_err(|_| AllocationError::Corrupt)?
        .checked_add(1)
        .ok_or(AllocationError::Corrupt)?;
    let event = SandboxEvent {
        sandbox_id: snapshot.sandbox.id.clone(),
        seq: seq as u64,
        appended_at: stamp.clone(),
        kind: SandboxEventKind::StatusChanged {
            from: SandboxStatus::Retiring,
            to: SandboxStatus::Retired,
        },
    };
    majority_collection::<Document>(&database, "sandbox_events")
        .insert_one(
            crate::store::sandbox_events::event_document(&event)
                .map_err(|_| AllocationError::Corrupt)?,
        )
        .session(&mut *session)
        .await?;
    let mut fields = crate::store::sandboxes::sandbox_replacement_fields(&snapshot.sandbox)
        .map_err(|_| AllocationError::Corrupt)?;
    fields.insert("last_seq", seq);
    fields.insert("channel_retirement_finalization", receipt_bson);
    fields.insert("channel_retirement_finalization_sha256", receipt_sha256);
    fields.insert("channel_history_seal", receipt.history.digest()?);
    if majority_collection::<Document>(&database, "sandboxes").update_one(
        doc! {"_id": snapshot.sandbox.id.as_str(), "revision": previous as i64, "channel_retirement_finalization": {"$exists": false}},
        doc! {"$set": fields}).session(&mut *session).await?.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    permit.require_current()?;
    snapshot.history.inventory_sealed = true;
    Ok(snapshot)
}

/// Ordinary readers validate the immutable retired record without acquiring a
/// cleanup permit or mutating a receipt. Existing retention guards still apply.
pub(in crate::store) fn validate_document(
    document: &Document,
    sandbox: &Sandbox,
) -> Result<(), AllocationError> {
    let Some(value) = document.get("channel_retirement_finalization") else {
        return if document.contains_key("channel_history_seal") {
            Err(AllocationError::Corrupt)
        } else {
            Ok(())
        };
    };
    let receipt: FinalizationReceipt = mongodb::bson::deserialize_from_bson(value.clone())
        .map_err(|_| AllocationError::Corrupt)?;
    receipt.require(sandbox)?;
    if document
        .get_str("channel_retirement_finalization_sha256")
        .ok()
        != Some(receipt.digest()?.as_str())
        || document.get_str("channel_history_seal").ok() != Some(receipt.history.digest()?.as_str())
    {
        return Err(AllocationError::Corrupt);
    }
    Ok(())
}
fn digest(value: &impl Serialize) -> Result<String, AllocationError> {
    layer2_core::profiles::content_digest(&("channel-retirement-finalization-v1", value))
        .map_err(|_| AllocationError::Corrupt)
}
