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
        self.charge(key, per_minute, 1).await
    }

    /// Reserve credits atomically before a process can spend them locally.
    pub(crate) async fn charge(
        &self,
        key: &str,
        per_minute: u32,
        tokens: u32,
    ) -> Result<Option<u64>, StoreError> {
        if tokens == 0 || tokens > per_minute.max(1) {
            return Err(StoreError::Malformed(
                "invalid request budget charge".into(),
            ));
        }
        let interval = i64::from(60_000_u32.div_ceil(per_minute.max(1)));
        let tolerance =
            interval * i64::from(per_minute.saturating_sub(1)) - interval * i64::from(tokens - 1);
        let increment = interval * i64::from(tokens);
        for _ in 0..8 {
            let result = self.limits.find_one_and_update(doc! {
                "_id": key,
                "$expr": { "$lte": ["$tat", { "$add": [{ "$toLong": "$$NOW" }, tolerance] }] },
            }, vec![doc! { "$set": {
                "tat": { "$add": [{ "$max": ["$tat", { "$toLong": "$$NOW" }] }, increment] },
                "expires_at": { "$add": ["$$NOW", 120_000] },
            } }]).return_document(ReturnDocument::After).await.map_err(unavailable)?;
            if result.is_some() {
                return Ok(None);
            }
            if let Some(delay) = self.retry_after(key, tolerance).await? {
                // A concurrent insert or refill may have made room since the
                // failed admission. Retry its atomic predicate before refusing.
                if delay == 0 {
                    continue;
                }
                return Ok(Some(delay));
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

    async fn retry_after(&self, key: &str, tolerance: i64) -> Result<Option<u64>, StoreError> {
        // MongoDB returns the selected $max operand's BSON integer width. A
        // newly inserted or refilled bucket can select the Int32 lower bound.
        // The same server clock used by admission determines Retry-After.
        let mut rows = self.limits.aggregate([
            doc! { "$match": { "_id": key } },
            doc! { "$project": { "wait_ms": { "$max": [0, { "$subtract": ["$tat", { "$add": [{ "$toLong": "$$NOW" }, tolerance] }] }] } } },
        ]).await.map_err(unavailable)?;
        use futures_util::TryStreamExt;
        let Some(row) = rows.try_next().await.map_err(unavailable)? else {
            return Ok(None);
        };
        let wait = row
            .get_i64("wait_ms")
            .or_else(|_| row.get_i32("wait_ms").map(i64::from))
            .map_err(|_| StoreError::Malformed("invalid request budget".into()))?;
        Ok(Some((wait.max(0) as u64).div_ceil(1000)))
    }
}

fn unavailable(error: mongodb::error::Error) -> StoreError {
    StoreError::Unavailable(format!("request budget: {error}"))
}

#[cfg(test)]
#[path = "request_limits_tests.rs"]
mod tests;
