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

use super::durability::majority_collection;

const OPERATIONS: &str = "operation_claims";
const EXPIRY_INDEX: &str = "operation_claim_expiry";
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
    fingerprint: Binary,
    outcome: Option<Binary>,
    expires_at: DateTime,
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
        store
            .operations
            .create_index(expiry_index())
            .await
            .map_err(|error| StoreError::Unavailable(format!("operation claim index: {error}")))?;
        tracing::info!("operation claim store ready");
        Ok(store)
    }

    async fn held(&self, claim: &OperationClaim) -> Result<Option<OperationDoc>, StoreError> {
        self.operations
            .find_one(id_filter(claim))
            .await
            .map_err(|error| StoreError::Unavailable(format!("read operation claim: {error}")))
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
            if stored.expires_at.to_system_time() <= now {
                continue;
            }
            let fingerprint = fingerprint_bytes(&stored)?;
            if fingerprint != claim.fingerprint() {
                return Ok(OperationReservation::FingerprintMismatch);
            }
            return Ok(match stored.outcome {
                Some(outcome) => OperationReservation::Complete(outcome.bytes),
                None => OperationReservation::Pending,
            });
        }

        Err(StoreError::Unavailable(
            "operation claim changed repeatedly during reservation".into(),
        ))
    }

    async fn complete(&self, claim: &OperationClaim, outcome: Vec<u8>) -> Result<(), StoreError> {
        let stored_outcome = binary(outcome.clone());
        let result = self
            .operations
            .update_one(
                doc! {
                    "_id": id_bson(claim),
                    "fingerprint": binary(claim.fingerprint().to_vec()),
                    "outcome": Bson::Null,
                    "expires_at": {
                        "$eq": DateTime::from_system_time(claim.expires_at()),
                        "$gt": DateTime::now(),
                    },
                },
                doc! { "$set": { "outcome": stored_outcome } },
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
        if stored.expires_at != DateTime::from_system_time(claim.expires_at()) {
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
        fingerprint: binary(claim.fingerprint().to_vec()),
        outcome: None,
        expires_at: DateTime::from_system_time(claim.expires_at()),
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
        "expires_at": { "$lte": DateTime::from_system_time(now) },
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
mod tests {
    use futures_util::TryStreamExt as _;
    use layer2_core::SubjectId;

    use super::*;

    fn claim(key: &str, fingerprint: u8, expires_at: SystemTime) -> OperationClaim {
        OperationClaim::new(
            SubjectId("subject-a".into()),
            "create-resource",
            key,
            [fingerprint; 32],
            expires_at,
        )
    }

    #[test]
    fn expiry_is_driven_by_the_claim_deadline() {
        let expiry = expiry_index();
        assert_eq!(expiry.keys, doc! { "expires_at": 1 });
        assert_eq!(expiry.options.unwrap().expire_after, Some(Duration::ZERO));
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
        assert_eq!(fingerprint_bytes(&stored).unwrap(), held.fingerprint());
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

        let future = SystemTime::now() + Duration::from_secs(86_400);
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

        let expired = claim("expired", 3, SystemTime::UNIX_EPOCH);
        first.operations.insert_one(to_doc(&expired)).await.unwrap();
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

        client.database(&database_name).drop().await.unwrap();
    }
}
