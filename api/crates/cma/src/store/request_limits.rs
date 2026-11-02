//! Deployment-wide request budgets, serialized by MongoDB and its clock.
use layer2_core::StoreError;
use mongodb::{
    bson::{doc, Document},
    options::{IndexOptions, ReturnDocument},
    Collection, Database, IndexModel,
};
use std::time::Duration;

pub(crate) const COLLECTION: &str = "request_rate_limits";

pub(crate) struct MongoRequestLimits {
    limits: Collection<Document>,
}

impl MongoRequestLimits {
    pub(crate) async fn open(database: &Database) -> Result<Self, StoreError> {
        super::schema::initialize_collections(database, &[COLLECTION]).await?;
        let limits = super::durability::majority_collection(database, COLLECTION);
        limits
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "expires_at": 1 })
                    .options(IndexOptions::builder().expire_after(Duration::ZERO).build())
                    .build(),
            )
            .await
            .map_err(unavailable)?;
        Ok(Self { limits })
    }

    /// GCRA with one minute's burst. Returns a retry delay on exhaustion.
    /// Missing keys are inserted empty, then admitted by the same atomic path.
    /// Database failures never fall back to a fresh process-local allowance.
    pub(crate) async fn check(
        &self,
        key: &str,
        per_minute: u32,
    ) -> Result<Option<u64>, StoreError> {
        let interval = i64::from(60_000_u32.div_ceil(per_minute.max(1)));
        let tolerance = interval * i64::from(per_minute.saturating_sub(1));
        for _ in 0..8 {
            let result = self.limits.find_one_and_update(doc! {
                "_id": key,
                "$expr": { "$lte": ["$tat", { "$add": [{ "$toLong": "$$NOW" }, tolerance] }] },
            }, vec![doc! { "$set": {
                "tat": { "$add": [{ "$max": ["$tat", { "$toLong": "$$NOW" }] }, interval] },
                "expires_at": { "$add": ["$$NOW", 120_000] },
            } }]).return_document(ReturnDocument::After).await.map_err(unavailable)?;
            if result.is_some() {
                return Ok(None);
            }
            // The same server time used by admission also determines Retry-After.
            let mut rows = self.limits.aggregate([
                doc! { "$match": { "_id": key } },
                doc! { "$project": { "wait_ms": { "$max": [1, { "$subtract": ["$tat", { "$add": [{ "$toLong": "$$NOW" }, tolerance] }] }] } } },
            ]).await.map_err(unavailable)?;
            use futures_util::TryStreamExt;
            if let Some(row) = rows.try_next().await.map_err(unavailable)? {
                let wait = row
                    .get_i64("wait_ms")
                    .map_err(|_| StoreError::Malformed("invalid request budget".into()))?;
                return Ok(Some((wait.max(1) as u64).div_ceil(1000)));
            }
            // Concurrent inserts are harmless: only the winner creates the empty bucket.
            let inserted = self
                .limits
                .insert_one(doc! {
                    "_id": key, "document_schema_version": 1, "tat": 0_i64,
                    "expires_at": mongodb::bson::DateTime::now(),
                })
                .await;
            if let Err(error) = inserted {
                if !super::sandbox_mongo::is_duplicate_key(&error) {
                    return Err(unavailable(error));
                }
            }
        }
        Err(StoreError::Unavailable("request budget contention".into()))
    }
}

fn unavailable(error: mongodb::error::Error) -> StoreError {
    StoreError::Unavailable(format!("request budget: {error}"))
}

#[cfg(test)]
#[path = "request_limits_tests.rs"]
mod tests;
