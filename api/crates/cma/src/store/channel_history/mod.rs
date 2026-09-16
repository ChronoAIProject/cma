//! Allocation-pinned retain policy and append-only reference accounting.
//! Every reference is written with its source effect; no TTL or expiry exists.
mod assessment;
mod bindings;
mod control_bindings;
mod controls;
pub(crate) use control_bindings::control_record;
#[cfg(test)]
pub(crate) use controls::capture as assess_controls;
mod inventory;
mod material;
mod model;
mod seal;
use crate::store::{
    bridge_allocations::MutationError,
    durability::majority_collection,
    sandbox_mongo::{read_record, record_bson, DOCUMENT_SCHEMA_VERSION},
};
pub(crate) use assessment::assess;
pub(crate) use bindings::{accepted_command, outbox, partition_command, tombstone};
use layer2_core::bridge::allocation::{AllocationError, AllocationRecord};
#[cfg(test)]
pub(crate) use material::outbox as assess_outbox;
use model::{digest, Reference, COUNT, POLICY, REFERENCES};
pub(crate) use model::{
    policy, ChannelHistoryAssessment, ChannelHistoryPolicy, HistoryHold, ReferenceKind,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection, Database, IndexModel,
};
pub(crate) use seal::{seal, HistorySeal};

pub(crate) async fn initialize(database: &Database) -> Result<(), layer2_core::StoreError> {
    crate::store::schema::initialize_collections(database, &[REFERENCES]).await?;
    majority_collection::<Document>(database, REFERENCES)
        .create_index(
            IndexModel::builder()
                .keys(doc! {"allocation_id": 1_i32, "_id": 1_i32})
                .build(),
        )
        .await
        .map_err(|_| {
            layer2_core::StoreError::Unavailable("create Channel history reference index".into())
        })?;
    Ok(())
}

/// Called only for the document inserted by a new allocation reservation. A
/// replay/mutation must preserve absence on legacy allocations, never repin it.
pub(crate) fn new_document(
    allocation: &AllocationRecord,
    mut document: Document,
) -> Result<Document, AllocationError> {
    if allocation.revision != 1 || allocation.updated_at_ms != allocation.created_at_ms {
        return Err(AllocationError::InvalidTransition);
    }
    document.insert(
        POLICY,
        record_bson(&ChannelHistoryPolicy::capture(allocation)?)
            .map_err(|_| AllocationError::Corrupt)?,
    );
    document.insert(COUNT, 0_i64);
    Ok(document)
}

pub(crate) async fn seed(
    database: &Database,
    allocation: &AllocationRecord,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let rows: Collection<Document> = majority_collection(database, "bridge_allocations");
    let document = rows
        .find_one(doc! {"_id": &allocation.id})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Corrupt)?;
    append(
        database,
        allocation,
        ReferenceKind::FirstSource,
        &document,
        session,
    )
    .await
}

async fn append(
    database: &Database,
    allocation: &AllocationRecord,
    kind: ReferenceKind,
    target: &Document,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let references: Collection<Document> = majority_collection(database, REFERENCES);
    let reference = Reference::capture(allocation, kind, target)?;
    let id = reference.id()?;
    let rows: Collection<Document> = majority_collection(database, "bridge_allocations");
    let document = rows
        .find_one(doc! {"_id": &allocation.id})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Corrupt)?;
    let current = crate::store::bridge_allocations::decode_record(&document)?;
    if current.id != allocation.id
        || ChannelHistoryPolicy::capture(&current)? != ChannelHistoryPolicy::capture(allocation)?
    {
        return Err(AllocationError::IdentityMismatch.into());
    }
    let pinned = policy(&document, allocation)?.is_some();
    let count = match document.get(COUNT) {
        Some(value) => value
            .as_i64()
            .filter(|count| *count >= 0)
            .ok_or(AllocationError::Corrupt)?,
        None if !pinned => 0,
        None => return Err(AllocationError::Corrupt.into()),
    };
    if let Some(document) = references
        .find_one(doc! {"_id": &id})
        .session(&mut *session)
        .await?
    {
        if read_reference(&document)? != reference {
            return Err(AllocationError::IdentityMismatch.into());
        }
        return Ok(());
    }
    let next = count.checked_add(1).ok_or(AllocationError::Corrupt)?;
    // Retained management journals and late-upload tombstones are permanent
    // extensions. A material seal never grants authority to erase either.
    if document.contains_key("channel_history_seal")
        && !matches!(
            kind,
            ReferenceKind::PartitionCommand
                | ReferenceKind::LateUploadTombstone
                | ReferenceKind::ControlAlias
                | ReferenceKind::ControlDisposition
        )
    {
        return Err(AllocationError::InvalidTransition.into());
    }
    let mut filter = doc! {"_id": &allocation.id};
    filter.insert(
        COUNT,
        document
            .get(COUNT)
            .cloned()
            .unwrap_or(mongodb::bson::Bson::Null),
    );
    let changed = rows
        .update_one(filter, doc! {"$set": {COUNT: next}})
        .session(&mut *session)
        .await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    references.insert_one(doc! {"_id": id, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "allocation_id": &allocation.id, "record": record_bson(&reference).map_err(|_| AllocationError::Corrupt)?})
        .session(session).await?;
    Ok(())
}

fn read_reference(document: &Document) -> Result<Reference, AllocationError> {
    let reference: Reference = read_record(document).map_err(|_| AllocationError::Corrupt)?;
    if reference.schema_version != 1
        || document.get_str("_id").ok() != Some(reference.id()?.as_str())
        || document.get_str("allocation_id").ok() != Some(reference.allocation_id.as_str())
        || reference.target_id.is_empty()
        || reference.target_sha256.len() != 64
        || !reference
            .target_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    {
        return Err(AllocationError::Corrupt);
    }
    Ok(reference)
}
