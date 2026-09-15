use layer2_core::bridge::allocation::{AllocationError, AllocationIdentity, AllocationRecord};
use mongodb::bson::{doc, Document};

use crate::store::sandbox_mongo::{read_record, record_bson, DOCUMENT_SCHEMA_VERSION};

pub(super) fn record_document(record: &AllocationRecord) -> Result<Document, AllocationError> {
    record.validate()?;
    Ok(
        doc! {"_id":&record.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "bridge_id":record.identity.bridge_id.as_str(),"key_hash":record.identity.key.hash(),
        "key_version":i32::from(record.identity.key.version()),"generation":record.identity.generation as i64,
        "revision":record.revision as i64,"lease_fence":record.lease_fence as i64,
        "creates_sandbox":record.placement.creates_sandbox(),
        "record":record_bson(record).map_err(|_| AllocationError::Corrupt)?},
    )
}

pub(super) fn decode_record(document: &Document) -> Result<AllocationRecord, AllocationError> {
    let record: AllocationRecord = read_record(document).map_err(|_| AllocationError::Corrupt)?;
    let expected = record_document(&record)?;
    for field in [
        "_id",
        "document_schema_version",
        "bridge_id",
        "key_hash",
        "key_version",
        "generation",
        "revision",
        "lease_fence",
        "creates_sandbox",
    ] {
        if document.get(field) != expected.get(field) {
            return Err(AllocationError::Corrupt);
        }
    }
    Ok(record)
}

pub(super) fn head_document(record: &AllocationRecord) -> Result<Document, AllocationError> {
    Ok(
        doc! {"_id":record.identity.head_id()?,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "bridge_id":record.identity.bridge_id.as_str(),"key_hash":record.identity.key.hash(),
        "key_version":i32::from(record.identity.key.version()),"generation":record.identity.generation as i64,
        "allocation_id":&record.id},
    )
}

pub(super) fn decode_head(
    document: &Document,
    identity: &AllocationIdentity,
) -> Result<String, AllocationError> {
    let mut current = identity.clone();
    let generation = document
        .get_i64("generation")
        .map_err(|_| AllocationError::Corrupt)?;
    current.generation = u64::try_from(generation).map_err(|_| AllocationError::Corrupt)?;
    let id = current.id()?;
    if document.get_str("_id").ok() != Some(identity.head_id()?.as_str())
        || document.get_i64("document_schema_version").ok() != Some(DOCUMENT_SCHEMA_VERSION)
        || document.get_str("bridge_id").ok() != Some(identity.bridge_id.as_str())
        || document.get_str("key_hash").ok() != Some(identity.key.hash())
        || document.get_i32("key_version").ok() != Some(i32::from(identity.key.version()))
        || document.get_str("allocation_id").ok() != Some(id.as_str())
    {
        return Err(AllocationError::Corrupt);
    }
    Ok(id)
}
