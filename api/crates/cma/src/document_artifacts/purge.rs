//! Server-clock leases and resumable enumeration for deleted document cleanup.
use super::{
    lifecycle::{self, object_range},
    quota::{number, unavailable, IO, QUOTA_COLLECTION},
};
use futures_util::TryStreamExt;
use layer2_core::document_artifacts::*;
use mongodb::{
    bson::{doc, DateTime, Document},
    options::{ReadConcern, ReturnDocument, WriteConcern},
    Collection, Database,
};
use std::collections::BTreeSet;

mod release;
#[derive(Clone)]
pub(crate) struct MongoDocumentPurgeStore {
    rows: Collection<Document>,
    quotas: Collection<Document>,
}
#[cfg(test)]
impl super::mongo::MongoDocumentStore {
    pub(crate) fn purge(&self) -> MongoDocumentPurgeStore {
        MongoDocumentPurgeStore::new(&self.rows.client().database(&self.rows.namespace().db))
    }
}
impl MongoDocumentPurgeStore {
    pub(crate) fn new(database: &Database) -> Self {
        let options = mongodb::options::CollectionOptions::builder()
            .read_concern(ReadConcern::majority())
            .write_concern(WriteConcern::majority())
            .build();
        Self {
            rows: database.collection_with_options(super::LIFECYCLE_COLLECTION, options.clone()),
            quotas: database.collection_with_options(QUOTA_COLLECTION, options),
        }
    }
}
pub(super) fn claim_filter(lease: &PurgeLease) -> Result<Document, DocumentError> {
    if !valid_id(&lease.document.id, "doc_")
        || !valid_digest(&lease.document.owner_hash)
        || !super::quota_baseline::valid_activation(&lease.holder)
        || lease.fence == 0
        || lease.fence > i64::MAX as u64
    {
        return Err(DocumentError::Corrupt);
    }
    Ok(
        doc! {"_id":&lease.document.id,"owner_hash":&lease.document.owner_hash,"state":"deleting","purge.lease.holder":&lease.holder,"purge.lease.fence":lease.fence as i64,"$expr":{"$gt":["$purge.lease.expires_at","$$NOW"]}},
    )
}
fn available_filter() -> Document {
    doc! {"state":"deleting","$expr":{"$and":[{"$lte":["$purge.next_at","$$NOW"]},{"$lte":[{"$ifNull":["$purge.lease.expires_at",DateTime::from_millis(0)]},"$$NOW"]}]}}
}
fn decode(row: &Document) -> Result<PurgeClaimable, DocumentError> {
    let tombstone = lifecycle::tombstone(row)?.ok_or(DocumentError::Corrupt)?;
    let purge = row
        .get_document("purge")
        .map_err(|_| DocumentError::Corrupt)?;
    let position = purge
        .get("position")
        .cloned()
        .map(mongodb::bson::deserialize_from_bson)
        .transpose()
        .map_err(|_| DocumentError::Corrupt)?;
    let deleted_at = DateTime::parse_rfc3339_str(&tombstone.deleted_at)
        .map_err(|_| DocumentError::Corrupt)?
        .timestamp_millis();
    let pass = u8::try_from(number(purge, "pass")?).map_err(|_| DocumentError::Corrupt)?;
    if !matches!(pass, 1 | 2) {
        return Err(DocumentError::Corrupt);
    }
    Ok(PurgeClaimable {
        id: tombstone.id,
        owner_hash: tombstone.owner_hash,
        deleted_at,
        last_reserved_at: purge
            .get_datetime("last_reserved_at")
            .ok()
            .map(|d| d.timestamp_millis()),
        pass,
        next_at: purge
            .get_datetime("next_at")
            .map_err(|_| DocumentError::Corrupt)?
            .timestamp_millis(),
        position,
    })
}
#[async_trait::async_trait]
impl DocumentPurgeStore for MongoDocumentPurgeStore {
    async fn due(&self, _now: i64, limit: usize) -> Result<Vec<PurgeClaimable>, DocumentError> {
        if limit == 0 {
            return Ok(vec![]);
        }
        let rows: Vec<Document> = self
            .rows
            .find(available_filter())
            .sort(doc! {"purge.next_at":1,"_id":1})
            .limit(limit.min(64) as i64)
            .max_time(IO)
            .await
            .map_err(unavailable)?
            .try_collect()
            .await
            .map_err(unavailable)?;
        rows.iter().map(decode).collect()
    }
    async fn claim(
        &self,
        id: &str,
        holder: &str,
        _now: i64,
    ) -> Result<Option<PurgeLease>, DocumentError> {
        if !valid_id(id, "doc_") || !super::quota_baseline::valid_activation(holder) {
            return Err(DocumentError::Invalid);
        }
        let mut filter = available_filter();
        filter.insert("_id", id);
        let row = self.rows.find_one_and_update(filter,vec![doc! {"$set":{"purge.lease.holder":holder,"purge.lease.fence":{"$add":[{"$ifNull":["$purge.lease.fence",0_i64]},1_i64]},"purge.lease.expires_at":{"$add":["$$NOW",PURGE_LEASE.as_millis() as i64]},"purge.attempts":{"$add":[{"$ifNull":["$purge.attempts",0_i64]},1_i64]}}}]).return_document(ReturnDocument::After).max_time(IO).await.map_err(unavailable)?;
        let Some(row) = row else { return Ok(None) };
        let document = decode(&row)?;
        let lease = row
            .get_document("purge")
            .and_then(|p| p.get_document("lease"))
            .map_err(|_| DocumentError::Corrupt)?;
        Ok(Some(PurgeLease {
            document,
            holder: holder.into(),
            fence: number(lease, "fence")?,
            expires_at: lease
                .get_datetime("expires_at")
                .map_err(|_| DocumentError::Corrupt)?
                .timestamp_millis(),
        }))
    }
    async fn objects(&self, lease: &PurgeLease) -> Result<Vec<String>, DocumentError> {
        let Some(row) = self
            .rows
            .find_one(claim_filter(lease)?)
            .max_time(IO)
            .await
            .map_err(unavailable)?
        else {
            return Ok(vec![]);
        };
        let tombstone = lifecycle::tombstone(&row)?.ok_or(DocumentError::Corrupt)?;
        let after = match &lease.document.position {
            Some(PurgePosition::KnownObjects { after }) => Some(after.as_str()),
            _ => None,
        };
        let mut keys: BTreeSet<String> = tombstone
            .objects
            .into_iter()
            .filter(|key| after.is_none_or(|a| key.as_str() > a))
            .collect();
        let prefix = format!(
            "object:{}:{}:",
            lease.document.owner_hash, lease.document.id
        );
        let mut range = object_range(&lease.document.owner_hash, &lease.document.id);
        if let Some(after) = after {
            let hash = after
                .strip_prefix(&lease.document.prefix())
                .and_then(|s| s.strip_suffix(".md"))
                .filter(|s| valid_digest(s))
                .ok_or(DocumentError::Corrupt)?;
            range.insert("_id",doc! {"$gt":format!("{prefix}{hash}"),"$lt":format!("object:{}:{};",lease.document.owner_hash,lease.document.id)});
        }
        let rows: Vec<Document> = self
            .quotas
            .find(range)
            .sort(doc! {"_id":1})
            .limit((PURGE_KNOWN_BATCH + 1) as i64)
            .max_time(IO)
            .await
            .map_err(unavailable)?
            .try_collect()
            .await
            .map_err(unavailable)?;
        for row in rows {
            super::quota::validate_row(&row, "object")?;
            let hash = row
                .get_str("_id")
                .ok()
                .and_then(|id| id.strip_prefix(&prefix))
                .filter(|hash| valid_digest(hash))
                .ok_or(DocumentError::Corrupt)?;
            keys.insert(format!("{}{hash}.md", lease.document.prefix()));
        }
        Ok(keys.into_iter().take(PURGE_KNOWN_BATCH + 1).collect())
    }
    async fn advance(&self, lease: &PurgeLease, step: PurgeStep) -> Result<bool, DocumentError> {
        let mut values = doc! {"purge.lease.expires_at":DateTime::from_millis(0)};
        let mut unset = doc! {"purge.blocked":"","purge.lease.holder":""};
        if let Some(time) = lease.document.last_reserved_at {
            values.insert("purge.last_reserved_at", DateTime::from_millis(time));
        }
        match step {
            PurgeStep::AwaitSecondPass { next_at } => {
                values.insert("purge.pass", 2_i64);
                values.insert("purge.next_at", DateTime::from_millis(next_at));
                unset.insert("purge.position", "");
            }
            PurgeStep::Retry { next_at } => {
                values.insert("purge.next_at", DateTime::from_millis(next_at));
            }
            PurgeStep::Progress { position, next_at } => {
                values.insert(
                    "purge.position",
                    mongodb::bson::serialize_to_bson(&position)
                        .map_err(|_| DocumentError::Corrupt)?,
                );
                values.insert("purge.next_at", DateTime::from_millis(next_at));
            }
        }
        let result = self
            .rows
            .update_one(claim_filter(lease)?, doc! {"$set":values,"$unset":unset})
            .await
            .map_err(unavailable)?;
        Ok(result.matched_count == 1)
    }
    async fn release(&self, lease: &PurgeLease) -> Result<bool, DocumentError> {
        release::release(self, lease).await
    }
    async fn block(&self, lease: &PurgeLease, reason: &str) -> Result<bool, DocumentError> {
        if reason != "storage_deletion_unsupported" {
            return Err(DocumentError::Invalid);
        }
        let result=self.rows.update_one(claim_filter(lease)?,vec![doc! {"$set":{"purge.blocked":reason,"purge.next_at":{"$add":["$$NOW",300_000_i64]},"purge.lease.expires_at":DateTime::from_millis(0)}},doc! {"$unset":"purge.lease.holder"}]).await.map_err(unavailable)?;
        Ok(result.matched_count == 1)
    }
    async fn backlog(&self, now: i64) -> Result<PurgeBacklog, DocumentError> {
        let rows:Vec<Document>=self.rows.aggregate([doc! {"$match":{"state":"deleting"}},doc! {"$group":{"_id":null,"pending":{"$sum":1_i64},"oldest":{"$min":"$deletion.deleted_at"}}}]).max_time(IO).await.map_err(unavailable)?.try_collect().await.map_err(unavailable)?;
        let Some(row) = rows.first() else {
            return Ok(PurgeBacklog::default());
        };
        let oldest = row
            .get_str("oldest")
            .ok()
            .and_then(|s| DateTime::parse_rfc3339_str(s).ok())
            .ok_or(DocumentError::Corrupt)?
            .timestamp_millis();
        Ok(PurgeBacklog {
            pending_documents: number(row, "pending")?,
            oldest_age_seconds: now.saturating_sub(oldest).max(0) as u64 / 1000,
        })
    }
}
