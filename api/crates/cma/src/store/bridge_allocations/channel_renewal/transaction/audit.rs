//! Every credential effect intent/receipt retains the actual consenting actor
//! and exact grant digest without persisting login or runtime credential bytes.
use super::*;
use crate::store::sandbox_mongo::{record_bson, DOCUMENT_SCHEMA_VERSION};
use sha2::{Digest, Sha256};

pub(super) async fn append(
    database: &Database,
    snapshot: &ChannelRenewalSnapshot,
    change: &Change<'_>,
    lease: &BridgeControlPlaneLease,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let state = snapshot.renewal.as_ref().ok_or(AllocationError::Corrupt)?;
    let action = match change {
        Change::Admit { .. }
            if state.phase == crate::sandbox::channel_renewal::model::RenewalPhase::MintStarted =>
        {
            "mint_intent"
        }
        Change::Admit { .. } => "resume",
        Change::Mint { .. } => "mint_receipt",
        Change::Deliver { .. } => "delivery_receipt",
        Change::Revoke { .. } => "revocation_receipt",
        _ => return Err(AllocationError::InvalidTransition.into()),
    };
    let grant = serde_json::to_vec(lease.grant()).map_err(|_| AllocationError::Corrupt)?;
    let document = doc! {
        "_id":format!("{}:channel-renewal:{}", snapshot.sandbox.id, state.revision),
        "document_schema_version":DOCUMENT_SCHEMA_VERSION,"sandbox_id":snapshot.sandbox.id.as_str(),"allocation_id":&snapshot.allocation.id,
        "sandbox_created_at":&snapshot.sandbox.created_at,"owner":record_bson(lease.owner()).map_err(|_| AllocationError::Corrupt)?,
        "pin":record_bson(&snapshot.allocation.pin).map_err(|_| AllocationError::Corrupt)?,
        "human_actor":record_bson(&lease.actor_context().actor).map_err(|_| AllocationError::Corrupt)?,
        "human_subject":lease.actor_subject().as_str(),"grant_digest":format!("{:x}", Sha256::digest(grant)),
        "custody_revision":i64::try_from(lease.custody_revision()).map_err(|_| AllocationError::Corrupt)?,
        "revision":i64::try_from(state.revision).map_err(|_| AllocationError::Corrupt)?,
        "phase":record_bson(&state.phase).map_err(|_| AllocationError::Corrupt)?,"action":action,
        "generation":i64::try_from(state.generation).map_err(|_| AllocationError::Corrupt)?,
        "previous_provider_key_id":&state.previous.provider_key_id,
        "replacement_provider_key_id":state.replacement.as_ref().map(|key| key.provider_key_id.as_str()),
        "fence":i64::try_from(state.claim.fence).map_err(|_| AllocationError::Corrupt)?,
        "appended_at":&snapshot.sandbox.updated_at,
    };
    let rows: Collection<Document> =
        majority_collection(database, "channel_credential_renewal_audits");
    rows.insert_one(document).session(&mut *session).await?;
    Ok(())
}
