//! Majority-durable registration and uncached primary linearizable exclusion.
use super::{canonical_digest, Cutover, WorkerHistory};
use crate::records::{mongo::MIGRATIONS, RecordError};
use futures_util::{future::BoxFuture, TryStreamExt};
use mongodb::{
    bson::{doc, DateTime, Document},
    options::{
        Acknowledgment, CollectionOptions, ReadConcern, ReadPreference, SelectionCriteria,
        WriteConcern,
    },
    Collection, Database, IndexModel,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const USES: &str = "eg_worker_credential_uses";
pub const CUTOVERS: &str = "eg_owner_key_cutovers";
pub const CUTOVER_BARRIER: &str = "worker_tracking_cutover";

/// Even an importer with missing feature flags must not become an untracked
/// writer after cutover. Standalone development stores cannot enable this lane.
pub async fn refuse_untracked_after_cutover(db: &Database) -> Result<(), RecordError> {
    let hello = db.run_command(doc! {"hello":1}).await.map_err(failure)?;
    if hello.get_str("setName").is_err() {
        return Ok(());
    }
    let barrier = db
        .collection_with_options::<Document>(MIGRATIONS, options())
        .find_one(doc! {"_id":CUTOVER_BARRIER})
        .max_time(Duration::from_secs(2))
        .await
        .map_err(failure)?;
    if barrier.is_some() {
        return Err(RecordError::Rejected(
            "worker tracking is mandatory after the durable writer cutover".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Use {
    #[serde(rename = "_id")]
    sha256: String,
    first_used_at: DateTime,
    schema_version: i32,
}

#[derive(Clone, Debug)]
pub struct MongoWorkerHistory {
    uses: Collection<Use>,
    cutovers: Collection<Cutover>,
}

pub(super) fn options() -> CollectionOptions {
    CollectionOptions::builder()
        .selection_criteria(SelectionCriteria::ReadPreference(ReadPreference::Primary))
        .read_concern(ReadConcern::linearizable())
        .write_concern(
            WriteConcern::builder()
                .w(Acknowledgment::Majority)
                .journal(true)
                .build(),
        )
        .build()
}

pub(super) fn failure(_: mongodb::error::Error) -> RecordError {
    RecordError::Unavailable("worker credential history unavailable".into())
}

fn indexes_valid(indexes: &[IndexModel]) -> bool {
    indexes.len() == 1
        && indexes[0].keys == doc! {"_id":1}
        && indexes[0].options.as_ref().is_none_or(|o| {
            o.expire_after.is_none()
                && o.partial_filter_expression.is_none()
                && o.sparse != Some(true)
                && o.hidden != Some(true)
                && o.collation.is_none()
        })
}

impl MongoWorkerHistory {
    pub async fn open(db: Database) -> Result<Self, RecordError> {
        let hello = db.run_command(doc! {"hello":1}).await.map_err(failure)?;
        if hello.get_str("setName").is_err() {
            return Err(RecordError::Rejected(
                "worker credential authority requires a replica set".into(),
            ));
        }
        let status = db
            .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
            .await
            .map_err(failure)?;
        cma_service_auth::mongodb_authority::verify_shared_schema(
            &status,
            db.name(),
            &role(db.name(), false),
        )
        .map_err(|_| {
            RecordError::Rejected("worker history collection authority required".into())
        })?;
        if db
            .collection::<Document>(MIGRATIONS)
            .find_one(doc! {"_id":"worker_credentials","version":1})
            .await
            .map_err(failure)?
            .is_none()
        {
            return Err(RecordError::Unavailable(
                "worker credential migration required".into(),
            ));
        }
        for name in [USES, CUTOVERS] {
            let indexes: Vec<IndexModel> = db
                .collection::<Document>(name)
                .list_indexes()
                .await
                .map_err(failure)?
                .try_collect()
                .await
                .map_err(failure)?;
            if !indexes_valid(&indexes) {
                return Err(RecordError::Rejected(
                    "worker history requires exact permanent ID index; no TTL".into(),
                ));
            }
        }
        Ok(Self {
            uses: db.collection_with_options(USES, options()),
            cutovers: db.collection_with_options(CUTOVERS, options()),
        })
    }

    async fn read_use(&self, sha256: &str) -> Result<bool, RecordError> {
        let sha256 = canonical_digest(sha256)?;
        let row = self
            .uses
            .find_one(doc! {"_id":sha256})
            .max_time(Duration::from_secs(2))
            .await
            .map_err(failure)?;
        match row {
            None => Ok(false),
            Some(row)
                if row.sha256 == sha256
                    && row.schema_version == 1
                    && row.first_used_at.timestamp_millis() > 0 =>
            {
                Ok(true)
            }
            Some(_) => Err(RecordError::Malformed(
                "invalid worker credential history".into(),
            )),
        }
    }
}

impl WorkerHistory for MongoWorkerHistory {
    fn record<'a>(&'a self, sha256: &'a str) -> BoxFuture<'a, Result<(), RecordError>> {
        Box::pin(async move {
            let sha256 = canonical_digest(sha256)?;
            let operation = async {
                match self
                    .uses
                    .insert_one(Use {
                        sha256: sha256.into(),
                        first_used_at: DateTime::now(),
                        schema_version: 1,
                    })
                    .await
                {
                    Ok(_) => Ok(()),
                    Err(error) if matches!(error.kind.as_ref(),mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(e)) if e.code==11000) => {
                        if self.read_use(sha256).await? {
                            Ok(())
                        } else {
                            Err(failure(error))
                        }
                    }
                    Err(error) => Err(failure(error)),
                }
            };
            tokio::time::timeout(Duration::from_secs(4), operation)
                .await
                .map_err(|_| {
                    RecordError::Unavailable(
                        "worker registration timed out; handoff refused".into(),
                    )
                })?
        })
    }

    fn was_worker<'a>(&'a self, sha256: &'a str) -> BoxFuture<'a, Result<bool, RecordError>> {
        Box::pin(async move {
            tokio::time::timeout(Duration::from_secs(4), self.read_use(sha256))
                .await
                .map_err(|_| RecordError::Unavailable("worker classification timed out".into()))?
        })
    }

    fn cutover(&self) -> BoxFuture<'_, Result<Option<Cutover>, RecordError>> {
        Box::pin(async move {
            tokio::time::timeout(Duration::from_secs(4), async {
                let row = self
                    .cutovers
                    .clone_with_type::<Document>()
                    .find_one(doc! {"_id":"owner-lane"})
                    .max_time(Duration::from_secs(2))
                    .await
                    .map_err(failure)?;
                row.map(mongodb::bson::deserialize_from_document)
                    .transpose()
                    .map_err(|_| RecordError::Malformed("worker cutover attestation".into()))
            })
            .await
            .map_err(|_| RecordError::Unavailable("worker cutover read timed out".into()))?
        })
    }
}

pub fn role(database: &str, migration: bool) -> Document {
    let mut grants = vec![
        doc! {"resource":{"db":database,"collection":USES},"actions":if migration {vec!["find","insert","createCollection","collMod","listIndexes"]} else {vec!["find","insert","listIndexes"]}},
        doc! {"resource":{"db":database,"collection":CUTOVERS},"actions":if migration {vec!["find","insert","update","createCollection","collMod","listIndexes"]} else {vec!["find","listIndexes"]}},
    ];
    if migration {
        grants.push(doc!{"resource":{"db":database,"collection":MIGRATIONS},"actions":["find","insert","update","collMod"]});
    }
    doc! {"createRole":if migration {"cma_eg_worker_credentials_migrator"} else {"cma_eg_worker_credentials_runtime"},"privileges":grants,"roles":[]}
}

pub async fn migrate(db: &Database) -> Result<(), RecordError> {
    cma_service_migrations::marker(db, MIGRATIONS, "worker_credentials", "version", &[1], 1)
        .await
        .map_err(failure)?;
    let uses = doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,
    "required":["_id","first_used_at","schema_version"],"properties":{
    "_id":{"bsonType":"string","pattern":"^[0-9a-f]{64}$"},"first_used_at":{"bsonType":"date"},"schema_version":{"enum":[1]}}}};
    let cutovers = doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,
    "required":["_id","attestation_id","not_before","revision","writer_release","all_writers_tracked","retained_records_backfilled"],"properties":{
    "_id":{"bsonType":"string","pattern":"^(owner-lane|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$"},"attestation_id":{"bsonType":"string","minLength":36,"maxLength":36},
    "not_before":{"bsonType":"long","minimum":1},"revision":{"bsonType":"long","minimum":1},
    "writer_release":{"bsonType":"string","pattern":"^[0-9a-f]{40}$"},
    "all_writers_tracked":{"enum":[true]},"retained_records_backfilled":{"enum":[true]}}}};
    for (name, validator) in [(USES, uses), (CUTOVERS, cutovers)] {
        cma_service_migrations::collection(db, name, validator, true)
            .await
            .map_err(failure)?;
        cma_service_migrations::exact_indexes(db, name, vec![], indexes_valid)
            .await
            .map_err(failure)?;
    }
    cma_service_migrations::collection_with_predecessors(
        db,
        MIGRATIONS,
        crate::records::mongo::migration_validator(),
        false,
        crate::records::mongo::migration_predecessors(),
    )
    .await
    .map_err(failure)?;
    cma_service_migrations::update(
        db,
        MIGRATIONS,
        doc! {"_id":"worker_credentials"},
        doc! {"$set":{"version":1}},
        true,
        false,
    )
    .await
    .map_err(failure)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worker_runtime_cannot_remove_rewrite_or_attest_history() {
        let role = role("gateway", false);
        let grants = role.get_array("privileges").unwrap();
        assert_eq!(grants.len(), 2);
        let actions = |i: usize| {
            grants[i]
                .as_document()
                .unwrap()
                .get_array("actions")
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(actions(0), vec!["find", "insert", "listIndexes"]);
        assert_eq!(actions(1), vec!["find", "listIndexes"]);
    }
}

#[cfg(test)]
#[path = "mongo_tests.rs"]
mod integration_tests;
