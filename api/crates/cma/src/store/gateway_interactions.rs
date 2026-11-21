//! Majority-durable expiring opaque records. Channel records use another collection.
use layer2_core::{
    gateway_interactions::GatewayInteractionStore, GatewayRecord, StoreError, SubjectId,
};
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, DateTime},
    options::{IndexOptions, ReturnDocument},
    Client, Collection, IndexModel,
};
use serde::{Deserialize, Serialize};

use super::{
    durability::majority_collection,
    sandbox_mongo::is_duplicate_key,
    schema::{self, DOCUMENT_SCHEMA_VERSION},
};

const COLLECTION: &str = "gateway_interaction_records";

#[derive(Serialize, Deserialize)]
struct Record {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i64,
    owner: String,
    blob: Binary,
    version: i64,
    expires_at: DateTime,
    created_at: String,
    updated_at: String,
}
impl Record {
    fn value(self) -> Result<GatewayRecord, StoreError> {
        schema::require_current(self.document_schema_version)?;
        Ok(GatewayRecord {
            record_id: self.id,
            owner: SubjectId::new(self.owner),
            blob: self.blob.bytes,
            version: u64::try_from(self.version)
                .map_err(|_| StoreError::Malformed("invalid interaction version".into()))?,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

pub struct MongoGatewayInteractionStore {
    records: Collection<Record>,
}
impl MongoGatewayInteractionStore {
    pub(crate) async fn open(client: &Client, database: &str) -> Result<Self, StoreError> {
        let database = client.database(database);
        schema::initialize_collections(&database, &[COLLECTION]).await?;
        let records = majority_collection(&database, COLLECTION);
        records
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"expires_at":1})
                    .options(
                        IndexOptions::builder()
                            .expire_after(std::time::Duration::ZERO)
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(unavailable)?;
        Ok(Self { records })
    }

    async fn version(&self, id: &str) -> Result<u64, StoreError> {
        self.records
            .find_one(doc! {"_id":id})
            .await
            .map_err(unavailable)?
            .map(|r| r.value().map(|v| v.version))
            .transpose()
            .map(|v| v.unwrap_or(0))
    }
}

#[async_trait::async_trait]
impl GatewayInteractionStore for MongoGatewayInteractionStore {
    async fn put(
        &self,
        id: &str,
        owner: &SubjectId,
        blob: Vec<u8>,
        expected: Option<u64>,
        expires_at: i64,
    ) -> Result<GatewayRecord, StoreError> {
        let now = layer2_core::now_rfc3339();
        let expiry = DateTime::from_millis(
            expires_at
                .checked_mul(1000)
                .ok_or_else(|| StoreError::Malformed("invalid interaction expiry".into()))?,
        );
        let blob = Binary {
            subtype: BinarySubtype::Generic,
            bytes: blob,
        };
        if let Some(expected) = expected {
            let version = i64::try_from(expected).map_err(|_| StoreError::RevisionConflict {
                expected,
                actual: 0,
            })?;
            // Expiry and ownership cannot be changed by an update. An expired
            // handle never becomes valid again while TTL deletion catches up.
            let filter = doc! {"_id":id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,"version":version,"owner":owner.as_str(),"expires_at": {"$gt":DateTime::now(),"$eq":expiry}};
            let update = doc! {"$set":{"blob":blob,"updated_at":now},"$inc":{"version":1_i64}};
            let value = self
                .records
                .find_one_and_update(filter, update)
                .return_document(ReturnDocument::After)
                .await
                .map_err(unavailable)?;
            return match value {
                Some(record) => record.value(),
                None => Err(StoreError::RevisionConflict {
                    expected,
                    actual: self.version(id).await?,
                }),
            };
        }
        let record = Record {
            id: id.into(),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            owner: owner.as_str().into(),
            blob,
            version: 1,
            expires_at: expiry,
            created_at: now.clone(),
            updated_at: now,
        };
        match self.records.insert_one(&record).await {
            Ok(_) => record.value(),
            Err(e) if is_duplicate_key(&e) => Err(StoreError::RevisionConflict {
                expected: 0,
                actual: self.version(id).await?,
            }),
            Err(e) => Err(unavailable(e)),
        }
    }

    async fn get(&self, id: &str) -> Result<Option<GatewayRecord>, StoreError> {
        self.records
            .find_one(doc! {"_id":id,"expires_at":{"$gt":DateTime::now()}})
            .await
            .map_err(unavailable)?
            .map(Record::value)
            .transpose()
    }

    async fn delete(&self, id: &str, expected: u64) -> Result<bool, StoreError> {
        let version = i64::try_from(expected).map_err(|_| StoreError::RevisionConflict {
            expected,
            actual: 0,
        })?;
        let result = self.records.delete_one(doc! {"_id":id,"version":version,"document_schema_version":DOCUMENT_SCHEMA_VERSION,"expires_at":{"$gt":DateTime::now()}}).await.map_err(unavailable)?;
        if result.deleted_count == 1 {
            return Ok(true);
        }
        match self.get(id).await? {
            None => Ok(false),
            Some(record) => Err(StoreError::RevisionConflict {
                expected,
                actual: record.version,
            }),
        }
    }
}

fn unavailable(_: mongodb::error::Error) -> StoreError {
    StoreError::Unavailable("interaction record storage unavailable".into())
}
