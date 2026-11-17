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
    #[cfg(test)]
    test_clock_millis: Option<i64>,
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
        Ok(Self {
            limits,
            #[cfg(test)]
            test_clock_millis: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn set_test_clock_millis(&mut self, millis: i64) {
        self.test_clock_millis = Some(millis);
    }

    /// GCRA with one minute's burst. Returns a retry delay on exhaustion.
    /// Missing keys are created and admitted by the same atomic path.
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
        // Admission and its returned preimage must use the same clock.
        // Returning computed wait_ms avoids persisting a decision sidecar.
        let clock = doc! { "$toLong": "$$NOW" };
        #[cfg(test)]
        let clock = self
            .test_clock_millis
            .map_or(clock, |millis| doc! { "$literal": millis });
        let boundary = doc! { "$add": [clock.clone(), tolerance] };
        let eligible = doc! { "$lte": [{ "$ifNull": ["$tat", 0_i64] }, boundary.clone()] };
        for _ in 0..8 {
            let result = self.limits.find_one_and_update(doc! { "_id": key }, vec![doc! {
                "$set": {
                    "document_schema_version": { "$ifNull": ["$document_schema_version", 1_i32] },
                    "tat": { "$cond": [eligible.clone(), {
                        "$add": [{ "$max": [{ "$ifNull": ["$tat", 0_i64] }, clock.clone()] }, increment]
                    }, "$tat"] },
                    // The split is intentional: token accounting uses the injected
                    // clock when set, while TTL uses the real server clock so the
                    // TTL monitor cannot delete the bucket mid-test.
                    "expires_at": { "$cond": [eligible.clone(), { "$add": ["$$NOW", 120_000] }, "$expires_at"] },
                }
            }])
                .upsert(true)
                .return_document(ReturnDocument::Before)
                .projection(doc! { "_id": 0, "wait_ms": { "$max": [0_i64, {
                    "$subtract": [{ "$ifNull": ["$tat", 0_i64] }, boundary.clone()]
                }] } })
                .await;
            let row = match result {
                Ok(None) => return Ok(None), // The new bucket was admitted atomically.
                Ok(Some(row)) => row,
                Err(error) if super::sandbox_mongo::is_duplicate_key(&error) => continue,
                Err(error) => return Err(unavailable(error)),
            };
            let wait = row
                .get_i64("wait_ms")
                .or_else(|_| row.get_i32("wait_ms").map(i64::from))
                .map_err(|_| StoreError::Malformed("invalid request budget".into()))?;
            return Ok((wait > 0).then(|| (wait as u64).div_ceil(1000)));
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
