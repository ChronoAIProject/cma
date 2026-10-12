//! MongoDB persistence for opaque event-gateway channel records.

use futures_util::TryStreamExt as _;
use layer2_core::{GatewayRecord, GatewayRecordHead, GatewayRecordStore, StoreError, SubjectId};
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary},
    options::ReturnDocument,
    Client, Collection, IndexModel,
};
use serde::{Deserialize, Serialize};

use super::{
    durability::majority_collection,
    sandbox_mongo::is_duplicate_key,
    schema::{self, DOCUMENT_SCHEMA_VERSION},
};

pub(crate) const GATEWAY_CHANNEL_RECORDS: &str = "gateway_channel_records";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GatewayRecordDoc {
    #[serde(rename = "_id")]
    record_id: String,
    document_schema_version: i64,
    owner: String,
    blob: Binary,
    version: i64,
    created_at: String,
    updated_at: String,
}

impl GatewayRecordDoc {
    fn new(record_id: &str, owner: &SubjectId, blob: Vec<u8>, now: String) -> Self {
        Self {
            record_id: record_id.into(),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            owner: owner.as_str().into(),
            blob: Binary {
                subtype: BinarySubtype::Generic,
                bytes: blob,
            },
            version: 1,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    fn record(self) -> Result<GatewayRecord, StoreError> {
        schema::require_current(self.document_schema_version)?;
        Ok(GatewayRecord {
            record_id: self.record_id,
            owner: SubjectId::new(self.owner),
            blob: self.blob.bytes,
            version: u64::try_from(self.version)
                .map_err(|_| StoreError::Malformed("gateway record version is negative".into()))?,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }

    fn head(self) -> Result<GatewayRecordHead, StoreError> {
        schema::require_current(self.document_schema_version)?;
        Ok(GatewayRecordHead {
            record_id: self.record_id,
            owner: SubjectId::new(self.owner),
            version: u64::try_from(self.version)
                .map_err(|_| StoreError::Malformed("gateway record version is negative".into()))?,
            updated_at: self.updated_at,
        })
    }
}

/// Durable Mongo adapter for the layer-2 gateway record port.
pub struct MongoGatewayRecordStore {
    records: Collection<GatewayRecordDoc>,
}

impl MongoGatewayRecordStore {
    pub(crate) async fn open(client: &Client, database: &str) -> Result<Self, StoreError> {
        Self::open_namespace(client, database, GATEWAY_CHANNEL_RECORDS).await
    }

    pub(crate) async fn open_connections(
        client: &Client,
        database: &str,
    ) -> Result<Self, StoreError> {
        Self::open_namespace(client, database, "gateway_connection_records").await
    }

    async fn open_namespace(
        client: &Client,
        database: &str,
        collection: &str,
    ) -> Result<Self, StoreError> {
        let database = client.database(database);
        schema::initialize_collections(&database, &[collection]).await?;
        let records = majority_collection(&database, collection);
        records
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "owner": 1_i32, "_id": 1_i32 })
                    .build(),
            )
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("create gateway record owner index: {error}"))
            })?;
        Ok(Self { records })
    }

    async fn current_version(&self, record_id: &str) -> Result<u64, StoreError> {
        self.records
            .find_one(doc! { "_id": record_id })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("read gateway record version: {error}"))
            })?
            .map(|record| record.record().map(|record| record.version))
            .transpose()
            .map(|version| version.unwrap_or(0))
    }
}

#[async_trait::async_trait]
impl GatewayRecordStore for MongoGatewayRecordStore {
    async fn put(
        &self,
        record_id: &str,
        owner: &SubjectId,
        blob: Vec<u8>,
        expected_version: Option<u64>,
    ) -> Result<GatewayRecord, StoreError> {
        if expected_version.is_none() {
            let record = GatewayRecordDoc::new(record_id, owner, blob, layer2_core::now_rfc3339());
            return match self.records.insert_one(&record).await {
                Ok(_) => record.record(),
                Err(error) if is_duplicate_key(&error) => Err(StoreError::RevisionConflict {
                    expected: 0,
                    actual: self.current_version(record_id).await?,
                }),
                Err(error) => Err(StoreError::Unavailable(format!(
                    "insert gateway record: {error}"
                ))),
            };
        }
        let now = layer2_core::now_rfc3339();
        let version = expected_version
            .map(|version| {
                i64::try_from(version).map_err(|_| StoreError::RevisionConflict {
                    expected: version,
                    actual: 0,
                })
            })
            .transpose()?;
        let filter = doc! {
            "_id": record_id,
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "version": version.expect("expected version is present"),
        };
        let update = doc! {
            "$set": {
                "owner": owner.as_str(),
                "blob": Binary { subtype: BinarySubtype::Generic, bytes: blob },
                "updated_at": now,
            },
            "$inc": { "version": 1_i64 },
        };
        let result = self
            .records
            .find_one_and_update(filter, update)
            .return_document(ReturnDocument::After)
            .upsert(false)
            .await;
        match result {
            Ok(Some(record)) => record.record(),
            Ok(None) => Err(StoreError::RevisionConflict {
                expected: expected_version.unwrap_or(0),
                actual: self.current_version(record_id).await?,
            }),
            Err(error) if is_duplicate_key(&error) => Err(StoreError::RevisionConflict {
                expected: 0,
                actual: self.current_version(record_id).await?,
            }),
            Err(error) => Err(StoreError::Unavailable(format!(
                "write gateway record: {error}"
            ))),
        }
    }

    async fn get(&self, record_id: &str) -> Result<Option<GatewayRecord>, StoreError> {
        self.records
            .find_one(doc! { "_id": record_id })
            .await
            .map_err(|error| StoreError::Unavailable(format!("read gateway record: {error}")))?
            .map(GatewayRecordDoc::record)
            .transpose()
    }

    async fn delete(
        &self,
        record_id: &str,
        expected_version: Option<u64>,
    ) -> Result<bool, StoreError> {
        let filter = if let Some(expected) = expected_version {
            doc! {
                "_id": record_id,
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "version": i64::try_from(expected).map_err(|_| StoreError::RevisionConflict { expected, actual: 0 })?,
            }
        } else {
            doc! { "_id": record_id }
        };
        let result =
            self.records.delete_one(filter).await.map_err(|error| {
                StoreError::Unavailable(format!("delete gateway record: {error}"))
            })?;
        if result.deleted_count == 1 || expected_version.is_none() {
            return Ok(result.deleted_count == 1);
        }
        let actual = self.current_version(record_id).await?;
        if actual == 0 {
            Ok(false)
        } else {
            Err(StoreError::RevisionConflict {
                expected: expected_version.expect("checked above"),
                actual,
            })
        }
    }

    async fn list(
        &self,
        owner: Option<&SubjectId>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<GatewayRecordHead>, StoreError> {
        let mut clauses = Vec::new();
        if let Some(owner) = owner {
            clauses.push(doc! { "owner": owner.as_str() });
        }
        if let Some(after) = after {
            clauses.push(doc! { "_id": { "$gt": after } });
        }
        let filter = match clauses.len() {
            0 => doc! {},
            1 => clauses.pop().expect("one clause exists"),
            _ => doc! { "$and": clauses },
        };
        let mut cursor = self
            .records
            .find(filter)
            .sort(doc! { "_id": 1_i32 })
            .limit(i64::try_from(limit).unwrap_or(i64::MAX))
            .await
            .map_err(|error| StoreError::Unavailable(format!("list gateway records: {error}")))?;
        let mut rows = Vec::new();
        while let Some(row) = cursor.try_next().await.map_err(|error| {
            StoreError::Unavailable(format!("read gateway record list: {error}"))
        })? {
            rows.push(row.head()?);
        }
        Ok(rows)
    }
}
