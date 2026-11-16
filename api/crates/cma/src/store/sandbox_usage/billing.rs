//! Durable delivery claims and immutable provider requests on usage intervals.
use super::*;
use mongodb::options::ReturnDocument;

pub(crate) const DELIVERY_LEASE_SECONDS: i64 = 120;

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
        let row = self
            .sandbox_usage_events
            .find_one_and_update(
                doc! {
                    "billing_delivered": false,
                    "billing_retry_at": { "$lte": now }, "billing_lease_until": { "$lte": now },
                    "$expr": {"$and":[
                        {"$lte":["$billing_lease_until",server_seconds()]},
                        {"$lte":["$billing_retry_at",server_seconds()]},
                    ]},
                },
                vec![doc! { "$set": {
                    "billing_claim": {"$literal": &claim},
                    "billing_lease_until": {"$add":[server_seconds(),DELIVERY_LEASE_SECONDS]},
                    "billing_last_attempt_at": server_seconds(),
                    "billing_attempts": {"$add":[{"$ifNull":["$billing_attempts",0_i64]},1_i64]},
                }}],
            )
            .sort(doc! { "billing_retry_at": 1, "ended_at": 1, "_id": 1 })
            .return_document(ReturnDocument::After)
            .await
            .map_err(unavailable)?;
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
        now: i64,
    ) -> Result<(), SandboxStoreError> {
        let updated = self
            .sandbox_usage_events
            .update_one(
                doc! {
                    "_id": delivery.event.transaction_id(), "billing_claim": &delivery.claim,
                    "billing_payload": { "$exists": false }, "billing_delivered": false,
                    "billing_lease_until": { "$gt": now },
                    "$expr": {"$gt":["$billing_lease_until",server_seconds()]},
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

    /// A late worker cannot resurrect an expired or replaced delivery claim.
    pub(crate) async fn renew_usage_delivery(
        &self,
        delivery: &Delivery,
        now: i64,
    ) -> Result<(), SandboxStoreError> {
        self.update_usage_claim(delivery, now, false).await
    }

    pub(crate) async fn begin_usage_delivery(
        &self,
        delivery: &Delivery,
        now: i64,
    ) -> Result<(), SandboxStoreError> {
        self.update_usage_claim(delivery, now, true).await
    }

    async fn update_usage_claim(
        &self,
        delivery: &Delivery,
        now: i64,
        sending: bool,
    ) -> Result<(), SandboxStoreError> {
        let mut fields = doc! {"billing_lease_until":{"$max":["$billing_lease_until",
        {"$add":[server_seconds(),DELIVERY_LEASE_SECONDS]}]}};
        if sending {
            // Persist possible acceptance before the network call. A crash can
            // leave an unknown outcome, never an invented successful receipt.
            fields.insert("billing_error", "provider_outcome_unknown");
        }
        let updated = self
            .sandbox_usage_events
            .update_one(
                doc! {"_id":delivery.event.transaction_id(),"billing_claim":&delivery.claim,
                "billing_delivered":false,"billing_lease_until":{"$gt":now},
                "$expr":{"$gt":["$billing_lease_until",server_seconds()]}},
                vec![doc! {"$set":fields}],
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
        "billing_retry_at": {"$add":[server_seconds(),300_i64]}, "billing_last_result_at": server_seconds() };
        // A later verification error or duplicate rejection cannot disprove an
        // earlier possible acceptance. Retain custody until a positive receipt.
        let outcome = match error {
            Some(error) => doc! {"$cond":[
                {"$eq":["$billing_error","provider_outcome_unknown"]},
                "provider_outcome_unknown", {"$literal":error},
            ]},
            None => doc! {"$literal":mongodb::bson::Bson::Null},
        };
        fields.insert("billing_error", outcome);
        let updated = self
            .sandbox_usage_events
            .update_one(
                doc! {
                    "_id": delivery.event.transaction_id(), "billing_claim": &delivery.claim,
                    "billing_delivered": false, "billing_lease_until": { "$gt": now },
                    "$expr": {"$gt":["$billing_lease_until",server_seconds()]},
                },
                vec![doc! { "$set": fields }],
            )
            .await
            .map_err(unavailable)?;
        if updated.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }
}

// Preserve the indexed caller-time predicates, but never let a skewed or
// rolled-back worker clock claim early or revive an already expired token.
fn server_seconds() -> mongodb::bson::Document {
    doc! {"$toLong":{"$divide":[{"$toLong":"$$NOW"},1000_i64]}}
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
