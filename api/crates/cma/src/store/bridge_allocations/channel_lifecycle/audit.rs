use super::*;
use crate::store::sandbox_mongo::{record_bson, DOCUMENT_SCHEMA_VERSION};
use mongodb::Database;
use sha2::{Digest, Sha256};

pub(super) async fn append(
    database: &Database,
    snapshot: &ChannelLifecycleSnapshot,
    permit: &ChannelRevivalAuthority,
    change: &LifecycleCredentialChange<'_>,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let state = snapshot
        .credential
        .as_ref()
        .ok_or(AllocationError::Corrupt)?;
    let action = match change {
        LifecycleCredentialChange::MintIntent(_) => "mint_intent",
        LifecycleCredentialChange::Mint { .. } => "mint_receipt",
        LifecycleCredentialChange::Deliver => "delivery_receipt",
        LifecycleCredentialChange::Revoke { .. } => "revocation_receipt",
        LifecycleCredentialChange::Publish => "publish_active",
        LifecycleCredentialChange::Read => return Ok(()),
    };
    let human = permit.human();
    let grant = serde_json::to_vec(human.grant()).map_err(|_| AllocationError::Corrupt)?;
    let row = doc! {"_id":format!("{}:{}:{}",snapshot.sandbox.id,state.operation.operation_id,action),"document_schema_version":DOCUMENT_SCHEMA_VERSION,
    "sandbox_id":snapshot.sandbox.id.as_str(),"allocation_id":&snapshot.allocation.id,"sandbox_created_at":&snapshot.sandbox.created_at,
    "operation":record_bson(&state.operation).map_err(|_| AllocationError::Corrupt)?,"runtime":record_bson(&state.runtime).map_err(|_| AllocationError::Corrupt)?,
    "owner":record_bson(human.owner()).map_err(|_| AllocationError::Corrupt)?,"pin":record_bson(&snapshot.allocation.pin).map_err(|_| AllocationError::Corrupt)?,
    "human_actor":record_bson(&human.actor_context().actor).map_err(|_| AllocationError::Corrupt)?,"human_subject":human.actor_subject().as_str(),"grant_digest":format!("{:x}",Sha256::digest(grant)),
    "custody_revision":human.custody_revision() as i64,"revision":state.revision as i64,"generation":state.generation as i64,"action":action,
    "phase":record_bson(&state.phase).map_err(|_| AllocationError::Corrupt)?,"fence":permit.sandbox().work.lease.as_ref().ok_or(AllocationError::LeaseUnavailable)?.fence as i64,
    "previous_provider_key_id":&state.previous.provider_key_id,"replacement_provider_key_id":state.replacement.as_ref().map(|key|key.provider_key_id.as_str()),"appended_at":&snapshot.sandbox.updated_at};
    let rows: Collection<Document> =
        majority_collection(database, "channel_lifecycle_credential_audits");
    rows.insert_one(row).session(&mut *session).await?;
    Ok(())
}
