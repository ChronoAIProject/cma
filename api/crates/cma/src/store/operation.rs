//! Durable operation claims and replay outcomes in MongoDB.

use std::time::{Duration, SystemTime};

use layer2_core::{OperationClaim, OperationReservation, OperationStore, StoreError};
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, Bson, DateTime},
    error::{Error, ErrorKind, WriteFailure},
    options::IndexOptions,
    Client, Collection, IndexModel,
};
use serde::{Deserialize, Serialize};

use super::{
    durability::majority_collection,
    schema::{self, DOCUMENT_SCHEMA_VERSION},
};

pub(super) const OPERATIONS: &str = "operation_claims";
const EXPIRY_INDEX: &str = "operation_outcome_expiry";
const DUPLICATE_KEY: i32 = 11_000;
const RESERVATION_RETRIES: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct OperationIdDoc {
    subject: String,
    operation: String,
    key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OperationDoc {
    #[serde(rename = "_id")]
    id: OperationIdDoc,
    document_schema_version: i64,
    #[serde(default)]
    claim_id: Option<String>,
    fingerprint: Binary,
    outcome: Option<Binary>,
    #[serde(default)]
    lease_expires_at: Option<DateTime>,
    #[serde(default)]
    recovery_required: bool,
    #[serde(default)]
    expires_at: Option<DateTime>,
}

/// MongoDB-backed operation claims.
#[derive(Clone)]
pub struct MongoOperationStore {
    operations: Collection<OperationDoc>,
}

impl MongoOperationStore {
    pub(super) async fn open(client: &Client, database: &str) -> Result<Self, StoreError> {
        let database = client.database(database);
        schema::initialize_collections(&database, &[OPERATIONS]).await?;
        let store = Self {
            operations: majority_collection(&database, OPERATIONS),
        };
        store.ensure_expiry_index().await?;
        tracing::info!("operation claim store ready");
        Ok(store)
    }

    async fn held(&self, claim: &OperationClaim) -> Result<Option<OperationDoc>, StoreError> {
        let document = self
            .operations
            .find_one(id_filter(claim))
            .await
            .map_err(|error| StoreError::Unavailable(format!("read operation claim: {error}")))?;
        if let Some(document) = &document {
            schema::require_current(document.document_schema_version)?;
        }
        Ok(document)
    }

    async fn ensure_expiry_index(&self) -> Result<(), StoreError> {
        self.operations
            .create_index(expiry_index())
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("operation outcome index: {error}"))
            })?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl OperationStore for MongoOperationStore {
    async fn reserve(&self, claim: &OperationClaim) -> Result<OperationReservation, StoreError> {
        for _ in 0..RESERVATION_RETRIES {
            let now = SystemTime::now();
            self.operations
                .delete_one(expired_filter(claim, now))
                .await
                .map_err(|error| {
                    StoreError::Unavailable(format!("expire operation claim: {error}"))
                })?;

            match self.operations.insert_one(to_doc(claim)).await {
                Ok(_) => return Ok(OperationReservation::Acquired),
                Err(error) if is_duplicate_key(&error) => {}
                Err(error) => {
                    return Err(StoreError::Unavailable(format!(
                        "reserve operation claim: {error}"
                    )))
                }
            }

            let Some(stored) = self.held(claim).await? else {
                // The expiry monitor may have removed the conflicting row
                // between the insert and read. Retry the acquisition rather
                // than reporting an operation that no longer exists.
                continue;
            };
            let fingerprint = fingerprint_bytes(&stored)?;
            if fingerprint != claim.fingerprint() {
                return Ok(OperationReservation::FingerprintMismatch);
            }
            if let Some(outcome) = stored.outcome {
                if stored
                    .expires_at
                    .is_some_and(|expires_at| expires_at.to_system_time() <= now)
                {
                    continue;
                }
                return Ok(OperationReservation::Complete(outcome.bytes));
            }
            if stored.recovery_required {
                return Ok(OperationReservation::RecoveryRequired);
            }
            if stored
                .lease_expires_at
                .is_some_and(|deadline| deadline.to_system_time() > now)
            {
                return Ok(OperationReservation::Pending);
            }

            // The execution owner disappeared or exceeded its finite lease.
            // Fence the claim durably before reporting recovery so no replica
            // can infer that lease expiry permits a second domain mutation.
            let result = self
                .operations
                .update_one(
                    recovery_filter(claim, now),
                    doc! { "$set": { "recovery_required": true } },
                )
                .await
                .map_err(|error| {
                    StoreError::Unavailable(format!("fence operation claim for recovery: {error}"))
                })?;
            if result.matched_count == 1 {
                return Ok(OperationReservation::RecoveryRequired);
            }
            // Completion or another recovery observer may have won between
            // the read and the fence. Re-read rather than guessing its state.
        }

        Err(StoreError::Unavailable(
            "operation claim changed repeatedly during reservation".into(),
        ))
    }

    async fn complete(&self, claim: &OperationClaim, outcome: Vec<u8>) -> Result<(), StoreError> {
        let stored_outcome = binary(outcome.clone());
        let expires_at = SystemTime::now()
            .checked_add(claim.outcome_retention())
            .ok_or_else(|| {
                StoreError::Malformed("operation outcome retention overflowed".into())
            })?;
        let result = self
            .operations
            .update_one(
                doc! {
                    "_id": id_bson(claim),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "claim_id": claim.claim_id(),
                    "fingerprint": binary(claim.fingerprint().to_vec()),
                    "outcome": Bson::Null,
                },
                doc! {
                    "$set": {
                        "outcome": stored_outcome,
                        "recovery_required": false,
                        "expires_at": DateTime::from_system_time(expires_at),
                    }
                },
            )
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("complete operation claim: {error}"))
            })?;
        if result.matched_count == 1 {
            return Ok(());
        }

        // A completion whose acknowledgement was lost is a normal retry. It
        // succeeds only when the already-recorded opaque bytes are identical.
        let stored = self.held(claim).await?.ok_or_else(|| {
            StoreError::Malformed("operation claim disappeared before completion".into())
        })?;
        if stored.claim_id.as_deref() != Some(claim.claim_id()) {
            return Err(StoreError::Malformed(
                "operation claim was replaced before completion".into(),
            ));
        }
        if fingerprint_bytes(&stored)? != claim.fingerprint() {
            return Err(StoreError::Malformed(
                "operation claim fingerprint changed before completion".into(),
            ));
        }
        match stored.outcome {
            Some(held) if held.bytes == outcome => Ok(()),
            Some(_) => Err(StoreError::Malformed(
                "operation claim already holds a different outcome".into(),
            )),
            None => Err(StoreError::Malformed(
                "operation claim expired before completion".into(),
            )),
        }
    }
}

fn to_doc(claim: &OperationClaim) -> OperationDoc {
    OperationDoc {
        id: operation_id(claim),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        claim_id: Some(claim.claim_id().to_string()),
        fingerprint: binary(claim.fingerprint().to_vec()),
        outcome: None,
        lease_expires_at: Some(DateTime::from_system_time(claim.lease_expires_at())),
        recovery_required: false,
        expires_at: None,
    }
}

fn operation_id(claim: &OperationClaim) -> OperationIdDoc {
    OperationIdDoc {
        subject: claim.subject().0.clone(),
        operation: claim.operation().to_string(),
        key: claim.key().to_string(),
    }
}

fn id_bson(claim: &OperationClaim) -> Bson {
    Bson::Document(doc! {
        "subject": claim.subject().as_str(),
        "operation": claim.operation(),
        "key": claim.key(),
    })
}

fn id_filter(claim: &OperationClaim) -> mongodb::bson::Document {
    doc! {
        "_id": id_bson(claim),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
    }
}

fn expired_filter(claim: &OperationClaim, now: SystemTime) -> mongodb::bson::Document {
    doc! {
        "_id": id_bson(claim),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "outcome": { "$type": "binData" },
        "expires_at": { "$lte": DateTime::from_system_time(now) },
    }
}

fn recovery_filter(claim: &OperationClaim, now: SystemTime) -> mongodb::bson::Document {
    doc! {
        "_id": id_bson(claim),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "fingerprint": binary(claim.fingerprint().to_vec()),
        "outcome": Bson::Null,
        "recovery_required": { "$ne": true },
        "$or": [
            { "lease_expires_at": { "$exists": false } },
            { "lease_expires_at": Bson::Null },
            { "lease_expires_at": { "$lte": DateTime::from_system_time(now) } },
        ],
    }
}

fn binary(bytes: Vec<u8>) -> Binary {
    Binary {
        subtype: BinarySubtype::Generic,
        bytes,
    }
}

fn fingerprint_bytes(stored: &OperationDoc) -> Result<&[u8; 32], StoreError> {
    stored.fingerprint.bytes.as_slice().try_into().map_err(|_| {
        StoreError::Malformed(format!(
            "operation claim fingerprint has {} bytes instead of 32",
            stored.fingerprint.bytes.len()
        ))
    })
}

fn expiry_index() -> IndexModel {
    IndexModel::builder()
        .keys(doc! { "expires_at": 1 })
        .options(
            IndexOptions::builder()
                .name(EXPIRY_INDEX.to_string())
                .expire_after(Duration::ZERO)
                .partial_filter_expression(doc! { "outcome": { "$type": "binData" } })
                .build(),
        )
        .build()
}

fn is_duplicate_key(error: &Error) -> bool {
    matches!(
        error.kind.as_ref(),
        ErrorKind::Write(WriteFailure::WriteError(write)) if write.code == DUPLICATE_KEY
    )
}

#[cfg(test)]
#[path = "operation_tests.rs"]
mod tests;
