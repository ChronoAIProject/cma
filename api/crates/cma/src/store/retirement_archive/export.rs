//! A consistent snapshot of the complete declared service namespace closure.
//! No projections discard record fields. Unknown external storage fails closed.
use super::*;
use crate::retirement_archive::{DatabaseRecord, ExternalRecord};
use layer2_core::document_artifacts::{DocumentBlobStore, StoredDocument};
use layer3_adapter::permanent_custody::PermanentCustody;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::time::Duration;
const MAX_DOCUMENTS: u64 = 2_000_000;
const MAX_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const CHUNK: usize = 4 * 1024 * 1024;

pub(crate) fn schema_sha256() -> String {
    digest(super::super::isolation::COLLECTIONS.join("\n").as_bytes())
}
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
    let ids = identities(&store.database, &request.sandbox_id, &mut session).await?;
    let filter = scope(&request.sandbox_id, &ids);
    let mut records = Vec::new();
    let mut external = Vec::new();
    let mut total = 0;
    for name in super::super::isolation::COLLECTIONS {
        let mut selected = filter.clone();
        if *name == "checkpoint_archive_tombstones" {
            selected = doc! {"$or":[filter.clone(),{"object_key":{"$regex":format!("^cma/checkpoints/{}/",digest(request.sandbox_id.as_bytes()))}}]};
        }
        let mut cursor = store
            .database
            .collection::<Document>(name)
            .find(selected)
            .sort(doc! {"_id":1})
            .batch_size(64)
            .max_time(Duration::from_secs(30))
            .session(&mut session)
            .await
            .map_err(|_| REFUSED)?;
        let mut record = DatabaseRecord {
            collection: (*name).into(),
            documents: 0,
            bytes: 0,
            sha256: String::new(),
            objects: Vec::new(),
        };
        let mut hasher = Sha256::new();
        let mut chunk = Vec::new();
        while cursor.advance(&mut session).await.map_err(|_| REFUSED)? {
            let mut document = cursor.deserialize_current().map_err(|_| REFUSED)?;
            if *name == "sandboxes" {
                document = original.clone();
            }
            // The retired checkpoint transport must not be silently reactivated.
            // Every source object must have a supported reader before retirement.
            if [
                "checkpoint_objects",
                "checkpoint_archive_tombstones",
                "binding_event_media_uploads",
            ]
            .contains(name)
            {
                return Err(REFUSED);
            }
            if *name == crate::document_artifacts::COLLECTION {
                external.extend(document_objects(&document, custody, blobs, request).await?);
            }
            let data = mongodb::bson::serialize_to_vec(&document).map_err(|_| REFUSED)?;
            record.documents += 1;
            record.bytes += data.len() as u64;
            total += data.len() as u64;
            if record.documents > MAX_DOCUMENTS || total > MAX_BYTES {
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
async fn identities(
    db: &Database,
    sandbox: &str,
    session: &mut ClientSession,
) -> Result<Vec<String>> {
    let mut ids = BTreeSet::from([sandbox.to_string()]);
    for name in ["workspaces", "worktrees", "agents"] {
        let mut cursor = db
            .collection::<Document>(name)
            .find(scope(sandbox, &ids.iter().cloned().collect::<Vec<_>>()))
            .session(&mut *session)
            .await
            .map_err(|_| REFUSED)?;
        while cursor.advance(&mut *session).await.map_err(|_| REFUSED)? {
            let document = cursor.deserialize_current().map_err(|_| REFUSED)?;
            let id = document.get_str("_id").map_err(|_| REFUSED)?;
            let (owner_parent, workspace, worktree) = match name {
                "workspaces" => {
                    let value =
                        super::super::workspaces::read_workspace(&document).map_err(|_| REFUSED)?;
                    (value.sandbox_id.to_string(), None, None)
                }
                "worktrees" => {
                    let value =
                        super::super::worktrees::read_worktree(&document).map_err(|_| REFUSED)?;
                    (
                        value.sandbox_id.to_string(),
                        Some(value.workspace_id.to_string()),
                        None,
                    )
                }
                "agents" => {
                    let value = super::super::agents::read_agent(&document).map_err(|_| REFUSED)?;
                    (
                        value.sandbox_id.to_string(),
                        Some(value.workspace_id.to_string()),
                        Some(value.worktree_id.to_string()),
                    )
                }
                _ => return Err(REFUSED),
            };
            if owner_parent != sandbox
                || workspace.as_ref().is_some_and(|id| !ids.contains(id))
                || worktree.as_ref().is_some_and(|id| !ids.contains(id))
            {
                return Err(REFUSED);
            }
            if !ids.insert(id.to_string()) || ids.len() > 100_000 {
                return Err(REFUSED);
            }
        }
    }
    Ok(ids.into_iter().collect())
}
fn scope(sandbox: &str, ids: &[String]) -> Document {
    let mut clauses = vec![doc! {"_id":{"$in":ids}}];
    for field in ["sandbox_id", "agent_id", "workspace_id", "worktree_id"] {
        for prefix in [
            "",
            "record.",
            "record.metadata.",
            "binding.",
            "target.",
            "record.target.",
        ] {
            let mut clause = Document::new();
            clause.insert(format!("{prefix}{field}"), doc! {"$in":ids});
            clauses.push(clause);
        }
    }
    for field in [
        "record.metadata.origin_agent_id",
        "record.runtime_author_agent_id",
    ] {
        let mut clause = Document::new();
        clause.insert(field, doc! {"$in":ids});
        clauses.push(clause);
    }
    clauses.push(doc! {"sandbox_id":sandbox});
    doc! {"$or":clauses}
}
async fn document_objects(
    document: &Document,
    custody: &dyn PermanentCustody,
    blobs: Option<&dyn DocumentBlobStore>,
    request: &Request,
) -> Result<Vec<ExternalRecord>> {
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
