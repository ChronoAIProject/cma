use super::{failure, RecordError, MIGRATIONS};
use crate::records::RecordClient;
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, Document},
    options::ReadConcern,
    Collection, Database, IndexModel,
};
use serde::Deserialize;

pub(super) const COLLECTION: &str = "eg_sender_stop_records";
pub(crate) const MARKER: &str = "sender_stop_records";
pub(super) const RUNTIME_ACTIONS: &[&str] = &["find", "insert", "update", "listIndexes"];
const VERSION: i32 = 1;

pub struct SenderStopRecords(Collection<SenderStopRecordV1>);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SenderStopRecordV1 {
    #[serde(rename = "_id")]
    id: String,
    schema_version: i32,
    pub revision: i64,
    pub stage: SenderStopStage,
    pub sealed_evidence: Binary,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SenderStopStage {
    Requested,
    BarrierRecorded,
    ReceiptRecorded,
}

impl SenderStopRecords {
    pub async fn open(client: &RecordClient) -> Result<Self, RecordError> {
        let db = client.database().ok_or_else(|| {
            RecordError::Rejected("sender stop records are unsupported by HTTP storage".into())
        })?;
        let invalid = || RecordError::Unavailable("sender stop storage is not ready".into());
        let status = db
            .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
            .await
            .map_err(failure)?;
        cma_service_auth::mongodb_authority::verify_shared_schema(
            &status,
            db.name(),
            &role(db.name()),
        )
        .map_err(|_| invalid())?;
        if !marker_installed(&db).await.map_err(failure)? {
            return Err(invalid());
        }
        let rows = db.collection::<SenderStopRecordV1>(COLLECTION);
        let indexes = rows
            .list_indexes()
            .await
            .map_err(failure)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(failure)?;
        if !indexes_valid(&indexes) {
            return Err(invalid());
        }
        Ok(Self(rows))
    }

    pub async fn load(&self, key: &str) -> Result<Option<SenderStopRecordV1>, RecordError> {
        let row = self
            .0
            .find_one(doc! {"_id":key})
            .read_concern(ReadConcern::majority())
            .await
            .map_err(failure)?;
        if row.as_ref().is_some_and(|row| {
            row.id != key
                || row.schema_version != VERSION
                || row.revision < 1
                || row.sealed_evidence.subtype != BinarySubtype::Generic
                || row.sealed_evidence.bytes.is_empty()
        }) {
            return Err(RecordError::Malformed("invalid sender stop record".into()));
        }
        Ok(row)
    }
}

fn role(database: &str) -> Document {
    doc! {"createRole":"cma_eg_sender_stop_runtime","roles":[],
    "privileges":[{"resource":{"db":database,"collection":COLLECTION},"actions":RUNTIME_ACTIONS}]}
}

pub(super) fn marker_validator() -> Document {
    let mut catalog = super::schema::migration_validator();
    catalog
        .get_array_mut("$or")
        .expect("marker alternatives")
        .push(doc! {"_id":MARKER,"version":VERSION}.into());
    catalog
}

pub(super) async fn marker_installed(db: &Database) -> mongodb::error::Result<bool> {
    let marker = db
        .collection::<Document>(MIGRATIONS)
        .find_one(doc! {"_id":MARKER})
        .read_concern(ReadConcern::majority())
        .await?;
    match marker {
        None => Ok(false),
        Some(row) if row.get_i32("version").ok() == Some(VERSION) => Ok(true),
        Some(_) => Err(mongodb::error::Error::custom(
            "unsupported sender stop marker",
        )),
    }
}

fn indexes_valid(indexes: &[IndexModel]) -> bool {
    matches!(indexes, [index] if index.keys == doc! {"_id":1}
    && index.options.as_ref().is_some_and(|options| {
        options.name.as_deref() == Some("_id_")
            && options.expire_after.is_none()
            && options.partial_filter_expression.is_none()
            && options.sparse != Some(true)
            && options.hidden != Some(true)
            && options.collation.is_none()
    }))
}

#[cfg(test)]
fn validator() -> Document {
    doc! {"$jsonSchema":{
        "bsonType":"object", "additionalProperties":false,
        "required":["_id","schema_version","revision","stage","sealed_evidence"],
        "properties":{
            "_id":{"bsonType":"string","pattern":r"\A[0-9a-f]{64}\z"},
            "schema_version":{"bsonType":"int","enum":[VERSION]},
            "revision":{"bsonType":"long","minimum":1},
            "stage":{"enum":["requested","barrier_recorded","receipt_recorded"]},
            "sealed_evidence":{"bsonType":"binData"}
        }
    }}
}

#[cfg(test)]
#[path = "mongo_sender_stop_tests.rs"]
mod tests;
