//! Durable delivery claims and immutable provider requests on usage intervals.
use super::*;
use mongodb::options::ReturnDocument;

pub(crate) struct Delivery {
    pub event: UsageEvent,
    pub claim: String,
    pub payload: Option<serde_json::Value>,
}

impl MongoSandboxStore {
    pub(crate) async fn claim_usage_delivery(
        &self,
        now: i64,
    ) -> Result<Option<Delivery>, SandboxStoreError> {
        let claim = uuid::Uuid::new_v4().to_string();
        let row = self.sandbox_usage_events.find_one_and_update(doc! {
            "billing_delivered": false,
            "billing_retry_at": { "$lte": now }, "billing_lease_until": { "$lte": now },
        }, doc! { "$set": { "billing_claim": &claim, "billing_lease_until": now + 120, "billing_last_attempt_at": now },
            "$inc": { "billing_attempts": 1_i64 } })
            .sort(doc! { "billing_retry_at": 1, "ended_at": 1, "_id": 1 })
            .return_document(ReturnDocument::After).await.map_err(unavailable)?;
        row.map(|row| {
            let payload = row
                .get_str("billing_payload")
                .ok()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            Ok(Delivery {
                event: read_event(&row)?,
                claim,
                payload,
            })
        })
        .transpose()
    }

    pub(crate) async fn freeze_usage_delivery(
        &self,
        delivery: &Delivery,
        payload: &serde_json::Value,
    ) -> Result<(), SandboxStoreError> {
        let updated = self
            .sandbox_usage_events
            .update_one(
                doc! {
                    "_id": delivery.event.transaction_id(), "billing_claim": &delivery.claim,
                    "billing_payload": { "$exists": false }, "billing_delivered": false,
                },
                doc! { "$set": { "billing_payload": payload.to_string() } },
            )
            .await
            .map_err(unavailable)?;
        if updated.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }

    pub(crate) async fn finish_usage_delivery(
        &self,
        delivery: &Delivery,
        now: i64,
        error: Option<&str>,
    ) -> Result<(), SandboxStoreError> {
        let mut fields = doc! { "billing_delivered": error.is_none(), "billing_lease_until": 0_i64,
        "billing_retry_at": now + 300, "billing_last_result_at": now };
        fields.insert(
            "billing_error",
            error.map_or(mongodb::bson::Bson::Null, Into::into),
        );
        let updated = self
            .sandbox_usage_events
            .update_one(
                doc! {
                    "_id": delivery.event.transaction_id(), "billing_claim": &delivery.claim,
                },
                doc! { "$set": fields },
            )
            .await
            .map_err(unavailable)?;
        if updated.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
