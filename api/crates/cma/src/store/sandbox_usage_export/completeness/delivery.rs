//! Separate outbox delivery. V1 usage events and their cursors retain their own chain.
use super::*;
use futures_util::TryStreamExt;
use mongodb::bson::DateTime;
use mongodb::options::ReturnDocument;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub(crate) struct CompletenessAttempt {
    pub id: String,
    pub digest: String,
    pub payload: Vec<u8>,
    pub generation: i64,
    destination: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompletenessReceipt {
    pub statement_id: String,
    pub receipt_id: String,
    pub statement_digest: String,
    pub accepted_at_ms: String,
    pub custody: String,
}

impl CompletenessReceipt {
    pub(crate) fn validates(&self, attempt: &CompletenessAttempt) -> bool {
        self.statement_id == attempt.id
            && self.statement_digest == attempt.digest
            && self.custody == "persisted"
            && uuid::Uuid::parse_str(&self.receipt_id)
                .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == self.receipt_id)
            && self.accepted_at_ms.parse::<u64>().is_ok_and(|value| {
                value > 0 && value <= i64::MAX as u64 && value.to_string() == self.accepted_at_ms
            })
    }
}

pub(crate) enum CompletenessResult {
    Accepted(CompletenessReceipt),
    Retry(u64),
    Hold,
}

impl CaptureStore {
    pub(crate) async fn completeness_delivery_page(
        &self,
        after: &str,
    ) -> Result<Vec<String>, SandboxStoreError> {
        if !self.completeness_enabled() {
            return Ok(vec![]);
        }
        let prefix = format!("{}/", hash(self.config.namespace.as_bytes()));
        let lower = format!("{prefix}{after}");
        let upper = format!("{}0", prefix.trim_end_matches('/'));
        let mut cursor = self.collection(INVENTORY).find(doc! {
            "_id":{"$gt":{"usage_completeness_outbox_v1":lower},"$lt":{"usage_completeness_outbox_v1":upper}},
        }).sort(doc! {"_id":1}).projection(doc! {"_id":1}).limit(32).await.map_err(unavailable)?;
        let mut rows = Vec::new();
        while let Some(row) = cursor.try_next().await.map_err(unavailable)? {
            let id = row
                .get_document("_id")
                .map_err(|_| corrupt())?
                .get_str("usage_completeness_outbox_v1")
                .map_err(|_| corrupt())?;
            let id = id.strip_prefix(&prefix).ok_or_else(corrupt)?;
            if !uuid::Uuid::parse_str(id)
                .is_ok_and(|value| value.get_version_num() == 4 && value.to_string() == id)
            {
                return Err(corrupt());
            }
            rows.push(id.into());
        }
        Ok(rows)
    }

    pub(crate) async fn claim_completeness(
        &self,
        id: &str,
        destination: &str,
    ) -> Result<Option<CompletenessAttempt>, SandboxStoreError> {
        let now = DateTime::now();
        let row = self.collection(INVENTORY).find_one_and_update(doc! {
            "_id":outbox_id(&self.config.namespace,id),"status":"pending",
            "$and":[{"$or":[{"destination":{"$exists":false}},{"destination":destination}]},
                {"$or":[{"due_at":{"$exists":false}},{"due_at":{"$lte":now}}]}],
        },doc! {"$inc":{"generation":1_i64},"$set":{"destination":destination,"due_at":DateTime::from_millis(now.timestamp_millis()+120_000)}})
            .return_document(ReturnDocument::After).await.map_err(unavailable)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let payload = row
            .get_binary_generic("canonical")
            .map_err(|_| corrupt())?
            .clone();
        let digest = row.get_str("digest").map_err(|_| corrupt())?.to_owned();
        let value: Value = serde_json::from_slice(&payload).map_err(|_| corrupt())?;
        let statement = Statement::from_value(value).map_err(|_| corrupt())?;
        if statement.statement_id != id
            || statement.producer_namespace != self.config.namespace
            || hash(&payload) != digest
            || serde_json_canonicalizer::to_vec(&statement).map_err(|_| corrupt())? != payload
        {
            return Err(corrupt());
        }
        Ok(Some(CompletenessAttempt {
            id: id.into(),
            digest,
            payload,
            generation: row.get_i64("generation").map_err(|_| corrupt())?,
            destination: destination.into(),
        }))
    }

    pub(crate) async fn finish_completeness(
        &self,
        attempt: &CompletenessAttempt,
        result: CompletenessResult,
    ) -> Result<(), SandboxStoreError> {
        let update = match result {
            CompletenessResult::Accepted(receipt) => {
                if !receipt.validates(attempt) {
                    return Err(corrupt());
                }
                doc! {"status":"accepted","receipt":records::encode(&receipt)?}
            }
            CompletenessResult::Retry(seconds) => {
                doc! {"due_at":DateTime::from_millis(DateTime::now().timestamp_millis()+(seconds.clamp(1,3600)*1000) as i64)}
            }
            CompletenessResult::Hold => doc! {"status":"held"},
        };
        let result = self.collection(INVENTORY).update_one(doc! {"_id":outbox_id(&self.config.namespace,&attempt.id),"generation":attempt.generation,
            "destination":&attempt.destination,"digest":&attempt.digest,"status":"pending"},doc! {"$set":update}).await.map_err(unavailable)?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }
}
