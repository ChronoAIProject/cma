#[path = "integration_store.rs"]
mod integrations;
pub(super) use integrations::RecordUpdate;
#[path = "binding_store.rs"]
mod bindings;
#[path = "store_dispatch.rs"]
mod dispatch;
use super::{
    schema::{runtime_role, COLLECTION},
    Error,
};
use crate::transaction_terminal::GuardedTerminal as _;
use base64::Engine;
use cmaeg_core::seal::{open_blob, seal_blob, MasterKeyRing};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, DateTime, Document},
    options::{Acknowledgment, CollectionOptions, ReadConcern, ReturnDocument, WriteConcern},
    Collection, Database, IndexModel,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::sync::Arc;
pub(super) struct Store {
    database: Database,
    raw: crate::records::mongo::MongoRecords,
    records: Collection<Row>,
    keys: Arc<MasterKeyRing>,
    pub(super) clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    #[serde(rename = "_id")]
    id: String,
    owner: String,
    kind: String,
    version: i64,
    schema_version: i32,
    #[serde(default)]
    guard_revision: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lease_until: Option<i64>,
    #[serde(default)]
    next_dispatch_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<DateTime>,
    blob: Binary,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sealed {
    id: String,
    owner: String,
    kind: String,
    value: serde_json::Value,
}
pub(super) struct NewRecord {
    id: String,
    kind: &'static str,
    value: serde_json::Value,
}
impl NewRecord {
    pub fn new(id: &str, kind: &'static str, value: &impl Serialize) -> Result<Self, Error> {
        Ok(Self {
            id: id.into(),
            kind,
            value: serde_json::to_value(value).map_err(|_| Error::Unavailable)?,
        })
    }
}
impl Store {
    pub async fn open(database: Database, keys: Arc<MasterKeyRing>) -> Result<Self, Error> {
        let hello = database
            .run_command(doc! {"hello":1})
            .await
            .map_err(|_| Error::Unavailable)?;
        if hello.get_str("setName").is_err() {
            return Err(Error::Unavailable);
        }
        let marker = database
            .collection::<Document>(crate::records::mongo::MIGRATIONS)
            .find_one(doc! {"_id":"github_integrations","version":6})
            .read_concern(ReadConcern::majority())
            .await
            .map_err(|_| Error::Unavailable)?;
        if marker.is_none() {
            return Err(Error::Unavailable);
        }
        let status = database
            .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
            .await
            .map_err(|_| Error::Unavailable)?;
        cma_service_auth::mongodb_authority::verify_shared_schema(
            &status,
            database.name(),
            &runtime_role(database.name()),
        )
        .map_err(|_| Error::Unavailable)?;
        let indexes: Vec<IndexModel> = database
            .collection::<Document>(COLLECTION)
            .list_indexes()
            .await
            .map_err(|_| Error::Unavailable)?
            .try_collect()
            .await
            .map_err(|_| Error::Unavailable)?;
        if indexes.len() != 4
            || !indexes
                .iter()
                .any(|index| index.keys == doc! {"owner":1,"kind":1,"_id":1})
            || !indexes.iter().any(|index| {
                index.keys == doc! {"expires_at":1}
                    && index.options.as_ref().is_some_and(|options| {
                        options.expire_after == Some(std::time::Duration::ZERO)
                    })
            })
            || indexes.iter().any(|index| {
                index.options.as_ref().is_some_and(|options| {
                    (index.keys != doc! {"expires_at":1} && options.expire_after.is_some())
                        || options.partial_filter_expression.is_some()
                        || options.sparse == Some(true)
                })
            })
            || !indexes
                .iter()
                .any(|index| index.keys == doc! {"owner":1,"kind":1,"next_dispatch_at":1,"_id":1})
        {
            return Err(Error::Unavailable);
        }
        let options = CollectionOptions::builder()
            .read_concern(ReadConcern::majority())
            .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
            .build();
        let raw = crate::records::mongo::MongoRecords::open_with_authority(
            database.clone(),
            cma_service_auth::mongodb_authority::MongoAuthorityMode::SharedSchema,
        )
        .await
        .map_err(|_| Error::Unavailable)?
        .for_kind(crate::records::RecordKind::Interaction);
        Ok(Self {
            records: database.collection_with_options(COLLECTION, options),
            raw,
            database,
            keys,
            clock: Arc::new(super::now),
        })
    }
    pub(super) fn current_time(&self) -> i64 {
        (self.clock)()
    }
    pub async fn create(
        &self,
        id: &str,
        owner: &str,
        kind: &str,
        value: &impl Serialize,
    ) -> Result<(), Error> {
        let row = self.row(id, owner, kind, 1, value)?;
        self.records
            .insert_one(row)
            .await
            .map_err(|_| Error::Unavailable)?;
        Ok(())
    }
    pub async fn create_many(&self, owner: &str, records: Vec<NewRecord>) -> Result<(), Error> {
        self.create_many_guarded(owner, records, None).await
    }
    pub async fn create_many_guarded(
        &self,
        owner: &str,
        records: Vec<NewRecord>,
        integration: Option<&super::AppIntegration>,
    ) -> Result<(), Error> {
        let rows = records
            .iter()
            .map(|record| self.row(&record.id, owner, record.kind, 1, &record.value))
            .collect::<Result<Vec<_>, _>>()?;
        let mut session = self
            .database
            .client()
            .start_session()
            .await
            .map_err(transaction_error)?;
        crate::transaction_start::start_write(&self.database, &mut session, None)
            .await
            .map_err(transaction_error)?;
        self.records
            .insert_many(rows)
            .session(&mut session)
            .await
            .map_err(transaction_error)?;
        if let Some(integration) = integration {
            self.guard_integration(owner, integration, &mut session)
                .await?;
        }
        session.commit_guarded().await.map_err(transaction_error)?;
        Ok(())
    }
    pub async fn source(
        &self,
        owner: &str,
        receipt: &super::Receipt,
        body: &[u8],
    ) -> Result<Vec<String>, Error> {
        let mut ids = Vec::new();
        for (index, chunk) in body.chunks(32 * 1024).enumerate() {
            let id = uuid::Uuid::new_v4().to_string();
            let bytes = serde_json::to_vec(&serde_json::json!({
                "kind": "github_source_chunk_v1",
                "receipt_id": receipt.receipt_id,
                "owner": owner,
                "body_sha256": receipt.body_sha256,
                "index": index,
                "count": body.len().div_ceil(32 * 1024),
                "expires_at": receipt.raw_body_expires_at,
                "bytes": base64::engine::general_purpose::STANDARD.encode(chunk),
            }))
            .map_err(|_| Error::Unavailable)?;
            let blob = cmaeg_core::seal::seal_interaction(&self.keys, &id, &bytes)
                .map_err(|_| Error::Unavailable)?;
            self.raw
                .put_expiring(&id, owner, &blob, None, Some(receipt.raw_body_expires_at))
                .await
                .map_err(|_| Error::Unavailable)?;
            ids.push(id);
        }
        Ok(ids)
    }
    fn row(
        &self,
        id: &str,
        owner: &str,
        kind: &str,
        version: i64,
        value: &impl Serialize,
    ) -> Result<Row, Error> {
        let value = serde_json::to_value(value).map_err(|_| Error::Unavailable)?;
        let expires_at = if kind == "delivery" {
            let received = value
                .pointer("/receipt/received_at")
                .and_then(serde_json::Value::as_i64)
                .ok_or(Error::Unavailable)?;
            let until = value
                .pointer("/receipt/retained_until")
                .and_then(serde_json::Value::as_i64)
                .ok_or(Error::Unavailable)?;
            let max_retention = receipt_retention_limit(
                value
                    .pointer("/receipt/status")
                    .and_then(serde_json::Value::as_str),
            );
            if until <= self.current_time()
                || until <= received
                || until.saturating_sub(received) > max_retention
            {
                return Err(Error::DeliveryExpired);
            }
            Some(DateTime::from_millis(
                until.checked_mul(1000).ok_or(Error::Unavailable)?,
            ))
        } else {
            None
        };
        let dispatch_pending = kind == "delivery" && delivery_pending(&value);
        let next_dispatch_at = delivery_next_attempt(&value).unwrap_or(0);
        let sealed = Sealed {
            id: id.into(),
            owner: owner.into(),
            kind: kind.into(),
            value,
        };
        let bytes = serde_json::to_vec(&sealed).map_err(|_| Error::Unavailable)?;
        let blob = seal_blob(&self.keys, &bytes).map_err(|_| Error::Unavailable)?;
        Ok(Row {
            id: id.into(),
            owner: owner.into(),
            kind: kind.into(),
            version,
            schema_version: 1,
            guard_revision: 0,
            lease_until: None,
            next_dispatch_at: if dispatch_pending {
                next_dispatch_at
            } else {
                i64::MAX
            },
            expires_at,
            blob: Binary {
                subtype: BinarySubtype::Generic,
                bytes: blob,
            },
        })
    }
    pub async fn commit_with_authority(
        &self,
        owner: &str,
        binding: &super::Binding,
        integration: &super::AppIntegration,
        expected_revision: u64,
        delivery: &super::Delivery,
    ) -> Result<(), Error> {
        let revision = i64::try_from(binding.version).map_err(|_| Error::Unavailable)?;
        let expected_revision = i64::try_from(expected_revision).map_err(|_| Error::Unavailable)?;
        let binding_row = self.row(&binding.binding_id, owner, "binding", revision, binding)?;
        let delivery_row =
            self.row(&delivery.receipt.receipt_id, owner, "delivery", 1, delivery)?;
        let mut session = self
            .database
            .client()
            .start_session()
            .await
            .map_err(transaction_error)?;
        crate::transaction_start::start_write(&self.database, &mut session, None)
            .await
            .map_err(transaction_error)?;
        let updated = self
            .records
            .update_one(
                doc! {"_id":&binding.binding_id,"owner":owner,"kind":"binding","version":expected_revision},
                guarded_update(binding_row),
            )
            .session(&mut session)
            .await
            .map_err(transaction_error)?;
        if updated.matched_count != 1 {
            return Err(Error::Retry);
        }
        self.records
            .insert_one(delivery_row)
            .session(&mut session)
            .await
            .map_err(transaction_error)?;
        self.guard_integration(owner, integration, &mut session)
            .await?;
        session.commit_guarded().await.map_err(transaction_error)?;
        Ok(())
    }
    pub async fn read<T: DeserializeOwned>(
        &self,
        id: &str,
        kind: &str,
    ) -> Result<(String, T), Error> {
        let (owner, _, value) = self.read_versioned(id, kind).await?;
        Ok((owner, value))
    }
    pub async fn read_owned<T: DeserializeOwned>(
        &self,
        owner: &str,
        id: &str,
        kind: &str,
    ) -> Result<(u64, T), Error> {
        let row = self
            .records
            .find_one(doc! {"_id":id,"kind":kind,"owner":owner})
            .await
            .map_err(|_| Error::Unavailable)?
            .ok_or(Error::NotFound)?;
        let (_, version, value) = self.open_row(row)?;
        Ok((version, value))
    }
    pub async fn read_versioned<T: DeserializeOwned>(
        &self,
        id: &str,
        kind: &str,
    ) -> Result<(String, u64, T), Error> {
        let row = self
            .records
            .find_one(doc! {"_id":id,"kind":kind})
            .await
            .map_err(|_| Error::Unavailable)?
            .ok_or(Error::NotFound)?;
        self.open_row(row)
    }
    fn open_row<T: DeserializeOwned>(&self, row: Row) -> Result<(String, u64, T), Error> {
        let opened = open_blob(&self.keys, &row.blob.bytes).map_err(|_| Error::Unavailable)?;
        let mut sealed: Sealed =
            serde_json::from_slice(&opened.payload).map_err(|_| Error::Unavailable)?;
        if sealed.id != row.id
            || sealed.owner != row.owner
            || sealed.kind != row.kind
            || row.schema_version != 1
            || row.version < 1
        {
            return Err(Error::Unavailable);
        }
        if row.kind == "delivery" {
            let until = row.expires_at.ok_or(Error::Unavailable)?.timestamp_millis() / 1000;
            if until <= self.current_time() {
                return Err(Error::DeliveryExpired);
            }
            let receipt = sealed
                .value
                .get_mut("receipt")
                .and_then(serde_json::Value::as_object_mut)
                .ok_or(Error::Unavailable)?;
            if receipt
                .get("retained_until")
                .is_some_and(|value| !value.is_null() && value.as_i64() != Some(until))
            {
                return Err(Error::Unavailable);
            }
            receipt.insert("retained_until".into(), serde_json::json!(until));
        } else if row.expires_at.is_some() {
            return Err(Error::Unavailable);
        }
        Ok((
            sealed.owner,
            u64::try_from(row.version).map_err(|_| Error::Unavailable)?,
            serde_json::from_value(sealed.value).map_err(|_| Error::Unavailable)?,
        ))
    }
    pub async fn list_owner<T: DeserializeOwned>(
        &self,
        owner: &str,
        kind: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<(String, u64, T)>, Error> {
        let mut filter = doc! {"owner":owner,"kind":kind};
        if let Some(after) = after {
            filter.insert("_id", doc! {"$gt":after});
        }
        let mut cursor = self
            .records
            .find(filter)
            .sort(doc! {"_id":1})
            .limit(i64::from(limit))
            .await
            .map_err(|_| Error::Unavailable)?;
        let mut values = Vec::new();
        while let Some(row) = cursor.try_next().await.map_err(|_| Error::Unavailable)? {
            let id = row.id.clone();
            let (_, version, value) = self.open_row(row)?;
            values.push((id, version, value));
        }
        Ok(values)
    }
}
fn receipt_retention_limit(status: Option<&str>) -> i64 {
    if status == Some("ignored") {
        7 * 86400
    } else {
        14 * 86400
    }
}
fn delivery_pending(value: &serde_json::Value) -> bool {
    value
        .pointer("/dispatch/channels")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|channels| {
            channels.iter().any(|channel| {
                channel.get("status").and_then(serde_json::Value::as_str) == Some("pending")
            })
        })
}
fn delivery_next_attempt(value: &serde_json::Value) -> Option<i64> {
    value
        .pointer("/dispatch/channels")
        .and_then(serde_json::Value::as_array)
        .and_then(|channels| {
            channels
                .iter()
                .filter(|channel| {
                    channel.get("status").and_then(serde_json::Value::as_str) == Some("pending")
                })
                .filter_map(|channel| {
                    channel
                        .get("next_attempt_at")
                        .and_then(serde_json::Value::as_i64)
                })
                .min()
        })
}
fn guarded_update(row: Row) -> Document {
    doc! {"$set":{"owner":row.owner,"kind":row.kind,"version":row.version,"schema_version":row.schema_version,"blob":row.blob},"$inc":{"guard_revision":1_i64}}
}
fn transaction_error(error: mongodb::error::Error) -> Error {
    if error.contains_label("TransientTransactionError")
        || matches!(error.kind.as_ref(),
        mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(write)) if write.code == 11000)
    {
        Error::Retry
    } else {
        Error::Unavailable
    }
}
