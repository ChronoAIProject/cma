//! A consistent snapshot of the complete declared service namespace closure.
//! No projections discard record fields. Unknown external storage fails closed.
use super::*;
use crate::retirement_archive::{DatabaseRecord, ExternalRecord};
use layer2_core::document_artifacts::{DocumentBlobStore, StoredDocument};
use layer3_adapter::permanent_custody::PermanentCustody;
use sha2::{Digest, Sha256};
use std::time::Duration;
const MAX_DOCUMENTS: u64 = 2_000_000;
const MAX_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const CHUNK: usize = 4 * 1024 * 1024;

pub(crate) async fn capture(
    store: &ArchiveStore,
    request: &Request,
    bytes: &[u8],
    custody: &dyn PermanentCustody,
    blobs: Option<&dyn DocumentBlobStore>,
) -> Result<(Vec<DatabaseRecord>, Vec<ExternalRecord>)> {
    let original = store.original(request, bytes).await?;
    let original: Document =
        mongodb::bson::deserialize_from_slice(&original).map_err(|_| REFUSED)?;
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(|_| REFUSED)?;
    let current = store
        .parents()
        .find_one(doc! {"_id":&request.sandbox_id})
        .read_concern(mongodb::options::ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|_| REFUSED)?
        .ok_or(REFUSED)?;
    let parent = super::super::sandboxes::read_sandbox(&current).map_err(|_| REFUSED)?;
    if parent.lifecycle.retirement_archive.is_some() {
        accepted(&current, request, bytes)?;
        if !parent.retirement_archive_pending() {
            return Err(REFUSED);
        }
    } else if digest(&mongodb::bson::serialize_to_vec(&current).map_err(|_| REFUSED)?)
        != request.source_sha256
    {
        return Err(REFUSED);
    }
    let closure =
        super::census::identities(&store.database, &request.sandbox_id, &mut session).await?;
    super::census::validate_catalog()?;
    let mut records = Vec::new();
    let mut external = Vec::new();
    let mut total = 0;
    let mut document_count = 0;
    let optional: Vec<_> = super::schema::manifest_collections()
        .iter()
        .copied()
        .filter(|name| super::super::isolation::OPTIONAL_COLLECTIONS.contains(name))
        .collect();
    let present_optional = store
        .database
        .list_collection_names()
        .filter(doc! {"name":{"$in":&optional}})
        .await
        .map_err(|_| REFUSED)?;
    for &name in super::schema::manifest_collections() {
        // Disabled features have no collection or grants. An absent optional
        // namespace contributes an explicit empty inventory without reading it.
        if optional.contains(&name)
            && !present_optional
                .iter()
                .any(|present| present.as_str() == name)
        {
            records.push(DatabaseRecord {
                collection: name.into(),
                documents: 0,
                bytes: 0,
                sha256: digest(&[]),
                objects: Vec::new(),
            });
            continue;
        }
        let selected =
            super::census::selection(&store.database, name, &closure, &mut session).await?;
        let mut cursor = store
            .database
            .collection::<mongodb::bson::RawDocumentBuf>(name)
            .find(selected)
            .sort(doc! {"_id":1})
            .batch_size(64)
            .max_time(Duration::from_secs(30))
            .session(&mut session)
            .await
            .map_err(|_| REFUSED)?;
        let mut record = DatabaseRecord {
            collection: name.into(),
            documents: 0,
            bytes: 0,
            sha256: String::new(),
            objects: Vec::new(),
        };
        let mut hasher = Sha256::new();
        let mut chunk = Vec::new();
        while cursor.advance(&mut session).await.map_err(|_| REFUSED)? {
            let raw = cursor.deserialize_current().map_err(|_| REFUSED)?;
            let document = Document::try_from(&raw).map_err(|_| REFUSED)?;
            if name == crate::document_artifacts::COLLECTION {
                external.extend(document_objects(&document, custody, blobs, request).await?);
            }
            let data = if name == "sandboxes" {
                mongodb::bson::serialize_to_vec(&original).map_err(|_| REFUSED)?
            } else {
                raw.as_bytes().to_vec()
            };
            record.documents += 1;
            document_count += 1;
            record.bytes += data.len() as u64;
            total += data.len() as u64;
            if document_count > MAX_DOCUMENTS || total > MAX_BYTES {
                return Err(REFUSED);
            }
            hasher.update(&data);
            // Whole BSON documents retain their native length framing and bytes.
            if chunk.len() + data.len() > CHUNK && !chunk.is_empty() {
                record.objects.push(
                    custody
                        .retain(&request.operation_id, std::mem::take(&mut chunk))
                        .await?,
                );
            }
            chunk.extend(data);
        }
        if !chunk.is_empty() {
            record
                .objects
                .push(custody.retain(&request.operation_id, chunk).await?);
        }
        record.sha256 = format!("{:x}", hasher.finalize());
        records.push(record);
    }
    external.sort_by(|a, b| a.key.cmp(&b.key));
    external.dedup();
    Ok((records, external))
}
async fn document_objects(
    document: &Document,
    custody: &dyn PermanentCustody,
    blobs: Option<&dyn DocumentBlobStore>,
    request: &Request,
) -> Result<Vec<ExternalRecord>> {
    if document.get_i64("document_schema_version").ok() != Some(1) {
        return Err(REFUSED);
    }
    let record: StoredDocument =
        mongodb::bson::deserialize_from_bson(document.get("record").ok_or(REFUSED)?.clone())
            .map_err(|_| REFUSED)?;
    record.validate().map_err(|_| REFUSED)?;
    let blobs = blobs.ok_or(REFUSED)?;
    let mut result = Vec::new();
    for revision in record.revisions {
        let data = blobs.get(&revision.object_key).await.map_err(|_| REFUSED)?;
        if digest(&data) != revision.metadata.sha256
            || data.len() as u64 != revision.metadata.size_bytes
        {
            return Err(REFUSED);
        }
        result.push(ExternalRecord {
            key: revision.object_key,
            sha256: revision.metadata.sha256,
            object: custody.retain(&request.operation_id, data).await?,
        });
    }
    Ok(result)
}

#[cfg(test)]
#[path = "export_tests.rs"]
mod tests;
