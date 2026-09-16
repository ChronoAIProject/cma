//! Corroborate retained state with its unique permanent original audit.
use super::*;
use crate::sandbox::channel_retirement::assessment::RetentionAuditIdentity;
use mongodb::Database;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StorageRetentionAudit {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i64,
    allocation_id: String,
    sandbox_id: layer2_core::SandboxId,
    sandbox_created_at: String,
    tree_fingerprint: String,
    operation_id: String,
    action: String,
    fence: i64,
    human_actor: layer2_core::resource_access::ResourceActor,
    receipt: crate::sandbox::channel_retirement::model::ChannelRetirementState,
    storage_retention: crate::sandbox::channel_retirement::retention::ChannelStorageRetention,
    appended_at: String,
}

pub(super) async fn verify(
    database: &Database,
    snapshot: &ChannelRetirementSnapshot,
    session: &mut ClientSession,
) -> Result<RetentionAuditIdentity, MutationError> {
    let rows: Collection<Document> = majority_collection(database, "channel_retirement_audits");
    // Bound uniqueness by allocation/action, so a second row with a different
    // root, timestamp or receipt cannot disappear behind an exact-match filter.
    let mut cursor = rows
        .find(doc! {"allocation_id":&snapshot.allocation.id,"action":"storage_retained"})
        .limit(2)
        .session(&mut *session)
        .await?;
    if !cursor.advance(&mut *session).await? {
        return Err(AllocationError::Corrupt.into());
    }
    let document = cursor.deserialize_current()?;
    if cursor.advance(&mut *session).await? {
        return Err(AllocationError::IdentityMismatch.into());
    }
    drop(cursor);
    let audit: StorageRetentionAudit = mongodb::bson::deserialize_from_document(document.clone())
        .map_err(|_| AllocationError::Corrupt)?;
    let retention = snapshot
        .retention
        .as_ref()
        .ok_or(AllocationError::Corrupt)?;
    let state = snapshot.state.as_ref().ok_or(AllocationError::Corrupt)?;
    let revision = audit
        .id
        .strip_prefix(&format!("{}:", snapshot.allocation.id))
        .and_then(|value| value.strip_suffix(":storage_retained"))
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or(AllocationError::IdentityMismatch)?;
    if audit.document_schema_version != crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION
        || revision == 0
        || revision > snapshot.sandbox.revision
        || audit.id != format!("{}:{revision}:storage_retained", snapshot.allocation.id)
        || audit.allocation_id != snapshot.allocation.id
        || audit.sandbox_id != snapshot.sandbox.id
        || audit.sandbox_created_at != snapshot.sandbox.created_at
        || audit.tree_fingerprint != snapshot.tree.fingerprint
        || audit.operation_id != snapshot.allocation.operation_id
        || audit.action != "storage_retained"
        || audit.fence <= 0
        || audit.human_actor != retention.original_human
        || audit.receipt != *state
        || audit.storage_retention != *retention
        || audit.appended_at != retention.recorded_at
    {
        return Err(AllocationError::IdentityMismatch.into());
    }
    let sha256 =
        layer2_core::profiles::content_digest(&("channel-final-retention-audit-v1", &document))
            .map_err(|_| AllocationError::Corrupt)?;
    Ok(RetentionAuditIdentity {
        id: audit.id,
        sha256,
    })
}
