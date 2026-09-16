//! Seal Channel archive writers while retaining every published archive.
use crate::{
    bridge::BridgeControlPlaneLease,
    sandbox::channel_retirement::{
        model::ChannelRetirementSnapshot,
        retention::{ChannelStorageRetention, RetainedArchive, MAX_RETAINED_ARCHIVES},
    },
    store::{
        bridge_allocations::MutationError, durability::majority_collection,
        schema::DOCUMENT_SCHEMA_VERSION,
    },
};
use layer2_core::bridge::allocation::AllocationError;
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection, Database,
};

pub(in crate::store) async fn seal(
    database: &Database,
    snapshot: &ChannelRetirementSnapshot,
    human: &BridgeControlPlaneLease,
    at: String,
    session: &mut ClientSession,
) -> Result<ChannelStorageRetention, MutationError> {
    if snapshot.retention.is_some() {
        return Err(AllocationError::InvalidTransition.into());
    }
    let id = snapshot.sandbox.id.as_str();
    let archives = inventory(database, id, session).await?;
    let retained = ChannelStorageRetention::capture(snapshot, human, archives, at)?;
    require_checkpoint_archives(&retained)?;
    let gates: Collection<Document> = majority_collection(database, super::model::GATES);
    gates
        .update_one(
            doc! { "_id": id },
            doc! { "$setOnInsert": {
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "retired": false, "serial": 0_i64,
            } },
        )
        .upsert(true)
        .session(&mut *session)
        .await?;
    // Admission and publication write this same gate inside their transactions.
    // An earlier in-flight upload can retain its cleanup stage but cannot publish.
    let changed = gates
        .update_one(
            doc! {
                "_id": id, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "retired": false, "channel_storage_retention": { "$exists": false },
            },
            doc! { "$set": {
                "retired": true, "channel_storage_retention": retained.digest()?,
            }, "$inc": { "serial": 1_i64 } },
        )
        .session(session)
        .await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::IdentityMismatch.into());
    }
    Ok(retained)
}

pub(in crate::store) async fn verify(
    database: &Database,
    retained: &ChannelStorageRetention,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let gates: Collection<Document> = majority_collection(database, super::model::GATES);
    let gate = gates
        .find_one(doc! {
            "_id": retained.sandbox_id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "retired": true, "channel_storage_retention": retained.digest()?,
        })
        .session(&mut *session)
        .await?;
    if gate.is_none()
        || inventory(database, retained.sandbox_id.as_str(), session).await? != retained.archives
    {
        return Err(AllocationError::IdentityMismatch.into());
    }
    require_checkpoint_archives(retained)?;
    Ok(())
}

async fn inventory(
    database: &Database,
    sandbox_id: &str,
    session: &mut ClientSession,
) -> Result<Vec<RetainedArchive>, MutationError> {
    let rows: Collection<Document> = majority_collection(database, super::model::MANIFESTS);
    let mut cursor = rows
        .find(doc! { "sandbox_id": sandbox_id, "state": "published" })
        .sort(doc! { "_id": 1 })
        .limit((MAX_RETAINED_ARCHIVES + 1) as i64)
        .session(&mut *session)
        .await?;
    let mut archives = Vec::new();
    while let Some(document) = cursor.next(&mut *session).await.transpose()? {
        if archives.len() == MAX_RETAINED_ARCHIVES {
            return Err(AllocationError::AuthorityUnavailable.into());
        }
        let manifest =
            super::model::read_manifest(document).map_err(|_| AllocationError::Corrupt)?;
        if manifest.sandbox_id != sandbox_id
            || manifest.state != super::model::ArchiveState::Published
        {
            return Err(AllocationError::IdentityMismatch.into());
        }
        archives.push(RetainedArchive {
            manifest_id: manifest.id,
            object_reference: manifest.object_reference,
            object_key: manifest.object_key,
            sha256: manifest.sha256,
            size_bytes: manifest.size_bytes as u64,
        });
    }
    Ok(archives)
}

fn require_checkpoint_archives(retained: &ChannelStorageRetention) -> Result<(), AllocationError> {
    for checkpoint in retained
        .checkpoints
        .latest
        .iter()
        .chain(retained.checkpoints.previous.iter())
    {
        if !retained.archives.iter().any(|archive| {
            archive.object_reference == checkpoint.object_reference
                && archive.sha256 == checkpoint.sha256
        }) {
            return Err(AllocationError::IdentityMismatch);
        }
    }
    Ok(())
}
