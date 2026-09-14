//! Encrypted OAuth custody. Only owner IDs and lease metadata are indexed.

use layer2_core::{StoreError, SubjectId};
use mongodb::{
    bson::{doc, DateTime},
    Client, Collection,
};
use serde::{Deserialize, Serialize};

use crate::{
    auth::persistence::SecretValue,
    connections::{
        subscription_store::{PendingLogin, RefreshLease, SubscriptionRecord, SubscriptionStore},
        SubscriptionError,
    },
};

use super::{
    durability::majority_collection,
    encryption::{AuthCipher, AuthEncryptionKey, CipherEnvelope},
    schema::{self, DOCUMENT_SCHEMA_VERSION},
};

const COLLECTION: &str = "codex_subscription_connections";
const KIND: &str = "codex-subscription";

#[derive(Serialize, Deserialize)]
struct SubscriptionDoc {
    #[serde(rename = "_id")]
    owner: String,
    document_schema_version: i64,
    incarnation: String,
    revision: u64,
    cache: Option<CipherEnvelope>,
    pending: Option<PendingDoc>,
    refresh_lease: Option<LeaseDoc>,
}

#[derive(Serialize, Deserialize)]
struct PendingDoc {
    intent: String,
    device: CipherEnvelope,
    expires_at: DateTime,
    heartbeat_at: DateTime,
    worker: String,
    state: layer2_core::trigger_domain::BrokerHandoffState,
}

#[derive(Serialize, Deserialize)]
struct LeaseDoc {
    holder: String,
    expires_at: DateTime,
}

pub(crate) struct MongoSubscriptionStore {
    records: Collection<SubscriptionDoc>,
    cipher: AuthCipher,
}

impl MongoSubscriptionStore {
    pub(super) async fn open(
        client: &Client,
        database: &str,
        deployment: &str,
        key: &AuthEncryptionKey,
    ) -> Result<Self, StoreError> {
        let database = client.database(database);
        schema::initialize_collections(&database, &[COLLECTION]).await?;
        Ok(Self {
            records: majority_collection(&database, COLLECTION),
            cipher: AuthCipher::new(deployment, key)?,
        })
    }

    fn encode(&self, record: SubscriptionRecord) -> Result<SubscriptionDoc, SubscriptionError> {
        if record.revision == 0
            || record.revision > i64::MAX as u64
            || uuid::Uuid::parse_str(&record.incarnation).is_err()
        {
            return Err(SubscriptionError::Changed);
        }
        let id = record.owner.as_str().as_bytes();
        let cache = record
            .cache
            .as_ref()
            .map(|cache| self.cipher.seal(KIND, id, "cache", cache.expose()))
            .transpose()
            .map_err(|_| SubscriptionError::Unavailable)?;
        let pending = record
            .pending
            .map(|pending| {
                let device = SecretValue::new(
                    serde_json::to_string(&pending.device)
                        .map_err(|_| SubscriptionError::InvalidAccount)?,
                );
                Ok::<_, SubscriptionError>(PendingDoc {
                    intent: pending.intent,
                    device: self
                        .cipher
                        .seal(KIND, id, "pending-device", device.expose())
                        .map_err(|_| SubscriptionError::Unavailable)?,
                    expires_at: DateTime::from_system_time(pending.expires_at),
                    heartbeat_at: DateTime::from_system_time(pending.heartbeat_at),
                    worker: pending.worker,
                    state: pending.state,
                })
            })
            .transpose()?;
        Ok(SubscriptionDoc {
            owner: record.owner.0,
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            incarnation: record.incarnation,
            revision: record.revision,
            cache,
            pending,
            refresh_lease: record.refresh_lease.map(|lease| LeaseDoc {
                holder: lease.holder,
                expires_at: DateTime::from_system_time(lease.expires_at),
            }),
        })
    }

    fn decode(&self, document: SubscriptionDoc) -> Result<SubscriptionRecord, SubscriptionError> {
        schema::require_current(document.document_schema_version)
            .map_err(|_| SubscriptionError::InvalidAccount)?;
        if document.revision == 0
            || document.revision > i64::MAX as u64
            || uuid::Uuid::parse_str(&document.incarnation).is_err()
        {
            return Err(SubscriptionError::InvalidAccount);
        }
        let id = document.owner.as_bytes();
        let cache = document
            .cache
            .map(|cache| {
                self.cipher
                    .open(KIND, id, "cache", cache)
                    .map(SecretValue::new)
            })
            .transpose()
            .map_err(|_| SubscriptionError::InvalidAccount)?;
        let pending = document
            .pending
            .map(|pending| {
                let secret = SecretValue::new(
                    self.cipher
                        .open(KIND, id, "pending-device", pending.device)
                        .map_err(|_| SubscriptionError::InvalidAccount)?,
                );
                Ok::<_, SubscriptionError>(PendingLogin {
                    intent: pending.intent,
                    device: serde_json::from_str(secret.expose())
                        .map_err(|_| SubscriptionError::InvalidAccount)?,
                    expires_at: pending.expires_at.to_system_time(),
                    heartbeat_at: pending.heartbeat_at.to_system_time(),
                    worker: pending.worker,
                    state: pending.state,
                })
            })
            .transpose()?;
        Ok(SubscriptionRecord {
            owner: SubjectId::new(document.owner),
            incarnation: document.incarnation,
            revision: document.revision,
            cache,
            pending,
            refresh_lease: document.refresh_lease.map(|lease| RefreshLease {
                holder: lease.holder,
                expires_at: lease.expires_at.to_system_time(),
            }),
        })
    }
}

#[async_trait::async_trait]
impl SubscriptionStore for MongoSubscriptionStore {
    async fn get(
        &self,
        owner: &SubjectId,
    ) -> Result<Option<SubscriptionRecord>, SubscriptionError> {
        self.records
            .find_one(doc! {"_id":owner.as_str()})
            .await
            .map_err(|_| SubscriptionError::Unavailable)?
            .map(|document| self.decode(document))
            .transpose()
    }

    async fn insert(&self, record: SubscriptionRecord) -> Result<bool, SubscriptionError> {
        if record.revision != 1 {
            return Err(SubscriptionError::Changed);
        }
        let owner = record.owner.clone();
        match self.records.insert_one(self.encode(record)?).await {
            Ok(_) => Ok(true),
            Err(_) => {
                if self.get(&owner).await?.is_some() {
                    Ok(false)
                } else {
                    Err(SubscriptionError::Unavailable)
                }
            }
        }
    }

    async fn replace(
        &self,
        expected: &SubscriptionRecord,
        next: SubscriptionRecord,
    ) -> Result<bool, SubscriptionError> {
        if next.owner != expected.owner
            || next.incarnation != expected.incarnation
            || expected.revision.checked_add(1) != Some(next.revision)
        {
            return Err(SubscriptionError::Changed);
        }
        let filter = doc! {"_id":expected.owner.as_str(), "incarnation": &expected.incarnation, "revision": expected.revision as i64};
        self.records
            .replace_one(filter, self.encode(next)?)
            .await
            .map(|result| result.matched_count == 1)
            .map_err(|_| SubscriptionError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::Arc,
        time::{Duration, SystemTime},
    };

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
    async fn mongo_subscription_encryption_and_cas_isolate_owners_and_replicas() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let client = Client::with_uri_str(&uri).await.unwrap();
        let other = Client::with_uri_str(&uri).await.unwrap();
        let database = format!("cma_subscription_test_{}", uuid::Uuid::new_v4().simple());
        let key =
            AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
        let first = Arc::new(
            MongoSubscriptionStore::open(&client, &database, "test", &key)
                .await
                .unwrap(),
        );
        let second = Arc::new(
            MongoSubscriptionStore::open(&other, &database, "test", &key)
                .await
                .unwrap(),
        );
        let owner = SubjectId::new("owner");
        let mut original = SubscriptionRecord::empty(owner.clone());
        original.cache = Some(SecretValue::new("fixture-renewable-cache".into()));
        original.pending = Some(PendingLogin {
            intent: uuid::Uuid::new_v4().to_string(),
            device: serde_json::from_value(serde_json::json!({"login_id":"fixture-native-login", "verification_url":"https://auth.openai.com/codex/device", "user_code":"ABCD-1234"})).unwrap(),
            heartbeat_at: SystemTime::now(), expires_at: SystemTime::now() + Duration::from_secs(600),
            worker: "replica-one".into(), state: layer2_core::trigger_domain::BrokerHandoffState::Pending,
        });
        assert!(first.insert(original.clone()).await.unwrap());
        assert!(!second.insert(original.clone()).await.unwrap());
        let raw = client
            .database(&database)
            .collection::<mongodb::bson::Document>(COLLECTION);
        let document = raw.find_one(doc! {"_id":"owner"}).await.unwrap().unwrap();
        let bytes = serde_json::to_vec(&document).unwrap();
        for secret in [
            "fixture-renewable-cache",
            "fixture-native-login",
            "ABCD-1234",
        ] {
            assert!(!bytes
                .windows(secret.len())
                .any(|part| part == secret.as_bytes()));
        }
        assert_eq!(
            second
                .get(&owner)
                .await
                .unwrap()
                .unwrap()
                .cache
                .unwrap()
                .expose(),
            "fixture-renewable-cache"
        );
        for cache in [true, false] {
            let mut copied = document.clone();
            copied.insert("_id", "another-owner");
            if !cache {
                copied.insert("cache", mongodb::bson::Bson::Null);
            }
            raw.replace_one(doc! {"_id":"another-owner"}, copied)
                .upsert(true)
                .await
                .unwrap();
            assert!(matches!(
                second.get(&SubjectId::new("another-owner")).await,
                Err(SubscriptionError::InvalidAccount)
            ));
        }
        let claim = |holder: &str| {
            let mut next = original.clone();
            next.revision += 1;
            next.refresh_lease = Some(RefreshLease {
                holder: holder.into(),
                expires_at: SystemTime::now() + Duration::from_secs(30),
            });
            next
        };
        let (left, right) = tokio::join!(
            first.replace(&original, claim("one")),
            second.replace(&original, claim("two"))
        );
        assert_ne!(left.unwrap(), right.unwrap());
        assert!(!first.replace(&original, claim("stale")).await.unwrap());
        let mut foreign = claim("foreign");
        foreign.owner = SubjectId::new("another-owner");
        assert!(matches!(
            first.replace(&original, foreign).await,
            Err(SubscriptionError::Changed)
        ));
        let other_deployment =
            MongoSubscriptionStore::open(&client, &database, "different-deployment", &key)
                .await
                .unwrap();
        assert!(matches!(
            other_deployment.get(&owner).await,
            Err(SubscriptionError::InvalidAccount)
        ));
        client.database(&database).drop().await.unwrap();
    }
}
