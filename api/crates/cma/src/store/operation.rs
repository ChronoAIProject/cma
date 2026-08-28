//! Durable operation claims and replay outcomes in MongoDB.

use std::time::{Duration, SystemTime};

use futures_util::TryStreamExt as _;
use layer2_core::{OperationClaim, OperationReservation, OperationStore, StoreError};
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, Bson, DateTime},
    error::{Error, ErrorKind, WriteFailure},
    options::IndexOptions,
    Client, Collection, IndexModel,
};
use serde::{Deserialize, Serialize};

use super::durability::majority_collection;

const OPERATIONS: &str = "operation_claims";
const LEGACY_EXPIRY_INDEX: &str = "operation_claim_expiry";
const EXPIRY_INDEX: &str = "operation_outcome_expiry";
const DUPLICATE_KEY: i32 = 11_000;
const NAMESPACE_NOT_FOUND: i32 = 26;
const INDEX_NOT_FOUND: i32 = 27;
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
        let store = Self {
            operations: majority_collection(&database, OPERATIONS),
        };
        store.ensure_expiry_index().await?;
        tracing::info!("operation claim store ready");
        Ok(store)
    }

    async fn held(&self, claim: &OperationClaim) -> Result<Option<OperationDoc>, StoreError> {
        self.operations
            .find_one(id_filter(claim))
            .await
            .map_err(|error| StoreError::Unavailable(format!("read operation claim: {error}")))
    }

    async fn ensure_expiry_index(&self) -> Result<(), StoreError> {
        let indexes = match self.operations.list_indexes().await {
            Ok(cursor) => match cursor.try_collect::<Vec<_>>().await {
                Ok(indexes) => indexes,
                Err(error) if is_namespace_not_found(&error) => Vec::new(),
                Err(error) => {
                    return Err(StoreError::Unavailable(format!(
                        "read operation claim indexes: {error}"
                    )))
                }
            },
            // A fresh database has no collection to inspect. Creating the
            // current index below creates that collection atomically.
            Err(error) if is_namespace_not_found(&error) => Vec::new(),
            Err(error) => {
                return Err(StoreError::Unavailable(format!(
                    "list operation claim indexes: {error}"
                )))
            }
        };
        if indexes.iter().any(|index| {
            index
                .options
                .as_ref()
                .and_then(|options| options.name.as_deref())
                == Some(LEGACY_EXPIRY_INDEX)
        }) {
            if let Err(error) = self.operations.drop_index(LEGACY_EXPIRY_INDEX).await {
                if !is_index_not_found(&error) {
                    return Err(StoreError::Unavailable(format!(
                        "replace legacy operation claim expiry index: {error}"
                    )));
                }
            }
        }
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
    doc! { "_id": id_bson(claim) }
}

fn expired_filter(claim: &OperationClaim, now: SystemTime) -> mongodb::bson::Document {
    doc! {
        "_id": id_bson(claim),
        "outcome": { "$type": "binData" },
        "expires_at": { "$lte": DateTime::from_system_time(now) },
    }
}

fn recovery_filter(claim: &OperationClaim, now: SystemTime) -> mongodb::bson::Document {
    doc! {
        "_id": id_bson(claim),
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

fn is_index_not_found(error: &Error) -> bool {
    matches!(error.kind.as_ref(), ErrorKind::Command(command) if command.code == INDEX_NOT_FOUND)
}

fn is_namespace_not_found(error: &Error) -> bool {
    matches!(error.kind.as_ref(), ErrorKind::Command(command) if command.code == NAMESPACE_NOT_FOUND)
}

#[cfg(test)]
mod tests {
    use futures_util::TryStreamExt as _;
    use layer2_core::SubjectId;

    use super::*;

    fn claim(key: &str, fingerprint: u8, lease_expires_at: SystemTime) -> OperationClaim {
        OperationClaim::new(
            SubjectId("subject-a".into()),
            "create-resource",
            key,
            [fingerprint; 32],
            lease_expires_at,
            Duration::from_secs(86_400),
        )
    }

    fn claim_with_lease(
        key: &str,
        fingerprint: u8,
        lease_expires_at: SystemTime,
        outcome_retention: Duration,
    ) -> OperationClaim {
        OperationClaim::new(
            SubjectId("subject-a".into()),
            "create-resource",
            key,
            [fingerprint; 32],
            lease_expires_at,
            outcome_retention,
        )
    }

    #[test]
    fn expiry_is_limited_to_completed_outcomes() {
        let expiry = expiry_index();
        assert_eq!(expiry.keys, doc! { "expires_at": 1 });
        let options = expiry.options.unwrap();
        assert_eq!(options.expire_after, Some(Duration::ZERO));
        assert_eq!(
            options.partial_filter_expression,
            Some(doc! { "outcome": { "$type": "binData" } })
        );
    }

    #[test]
    fn the_compound_identity_and_fingerprint_round_trip() {
        let held = claim(
            "retry-a",
            7,
            SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000),
        );
        let stored = to_doc(&held);
        assert_eq!(stored.id, operation_id(&held));
        assert_eq!(stored.claim_id.as_deref(), Some(held.claim_id()));
        assert_eq!(stored.expires_at, None);
        assert_eq!(fingerprint_bytes(&stored).unwrap(), held.fingerprint());
    }

    #[test]
    fn recovery_fence_requires_the_exact_unfinished_expired_lease() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        let held = claim_with_lease(
            "stale",
            4,
            now - Duration::from_secs(1),
            Duration::from_secs(86_400),
        );
        let filter = recovery_filter(&held, now);
        assert_eq!(filter.get("_id"), Some(&id_bson(&held)));
        assert_eq!(
            filter.get("fingerprint"),
            Some(&Bson::Binary(binary(held.fingerprint().to_vec())))
        );
        assert_eq!(filter.get("outcome"), Some(&Bson::Null));
        assert!(filter.contains_key("$or"));
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
    async fn real_mongo_reserves_once_replays_and_reuses_expired_keys() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
        let client = Client::with_uri_str(&uri).await.unwrap();
        let database_name = format!("chrono_operation_test_{}", uuid::Uuid::new_v4().simple());
        let first = MongoOperationStore::open(&client, &database_name)
            .await
            .unwrap();
        let second = MongoOperationStore::open(&client, &database_name)
            .await
            .unwrap();

        let indexes = client
            .database(&database_name)
            .collection::<mongodb::bson::Document>(OPERATIONS)
            .list_indexes()
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        let expiry = indexes
            .into_iter()
            .find(|index| {
                index
                    .options
                    .as_ref()
                    .and_then(|options| options.name.as_deref())
                    == Some(EXPIRY_INDEX)
            })
            .expect("named operation expiry index exists");
        assert_eq!(expiry.keys, doc! { "expires_at": 1 });
        assert_eq!(expiry.options.unwrap().expire_after, Some(Duration::ZERO));

        let future = SystemTime::now() + Duration::from_secs(120);
        let held = claim("held", 1, future);
        let (left, right) = tokio::join!(first.reserve(&held), second.reserve(&held));
        let reservations = [left.unwrap(), right.unwrap()];
        assert_eq!(
            reservations
                .iter()
                .filter(|result| matches!(result, OperationReservation::Acquired))
                .count(),
            1
        );
        assert_eq!(
            reservations
                .iter()
                .filter(|result| matches!(result, OperationReservation::Pending))
                .count(),
            1
        );

        first.complete(&held, b"created".to_vec()).await.unwrap();
        assert_eq!(
            second.reserve(&held).await.unwrap(),
            OperationReservation::Complete(b"created".to_vec())
        );
        assert_eq!(
            second.reserve(&claim("held", 2, future)).await.unwrap(),
            OperationReservation::FingerprintMismatch
        );

        let expired = claim_with_lease("expired", 3, future, Duration::ZERO);
        assert_eq!(
            first.reserve(&expired).await.unwrap(),
            OperationReservation::Acquired
        );
        first
            .complete(&expired, b"expired outcome".to_vec())
            .await
            .unwrap();
        let reusable = claim("expired", 3, future);
        assert_eq!(
            second.reserve(&reusable).await.unwrap(),
            OperationReservation::Acquired
        );
        assert!(first
            .complete(&expired, b"stale outcome".to_vec())
            .await
            .is_err());
        assert_eq!(
            second.reserve(&reusable).await.unwrap(),
            OperationReservation::Pending
        );
        second
            .complete(&reusable, b"current outcome".to_vec())
            .await
            .unwrap();

        let abandoned = claim_with_lease(
            "abandoned",
            5,
            SystemTime::UNIX_EPOCH,
            Duration::from_secs(86_400),
        );
        first
            .operations
            .insert_one(to_doc(&abandoned))
            .await
            .unwrap();
        assert_eq!(
            second.reserve(&abandoned).await.unwrap(),
            OperationReservation::RecoveryRequired
        );
        assert_eq!(
            first.reserve(&abandoned).await.unwrap(),
            OperationReservation::RecoveryRequired
        );
        first
            .complete(&abandoned, b"reconciled".to_vec())
            .await
            .unwrap();
        assert_eq!(
            second.reserve(&abandoned).await.unwrap(),
            OperationReservation::Complete(b"reconciled".to_vec())
        );

        client.database(&database_name).drop().await.unwrap();
    }
}
