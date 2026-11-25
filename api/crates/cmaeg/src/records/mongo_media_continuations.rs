//! Optional expiring active heads; base schema/startup does not require this index.
use super::{failure, MongoRecords, RecordError, RecordHead, MEDIA_CONTINUATIONS, MIGRATIONS};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    options::IndexOptions,
    Database, IndexModel,
};
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Head {
    #[serde(rename = "_id")]
    id: String,
    owner: String,
    version: i64,
    schema_version: i32,
}

pub fn role(database: &str, migration: bool) -> Document {
    let mut privileges = vec![doc! {
        "resource":{"db":database,"collection":MEDIA_CONTINUATIONS},
        "actions":if migration {
            vec!["find","insert","update","createCollection","collMod","createIndex","listIndexes"]
        } else { vec!["find","insert","update","remove","listIndexes"] }
    }];
    if migration {
        privileges.push(doc! {"resource":{"db":database,"collection":MIGRATIONS},"actions":["find","insert","update","collMod"]});
    }
    doc! {"createRole":if migration {"cma_eg_media_continuations_migrator"} else {"cma_eg_media_continuations_runtime"},"privileges":privileges,"roles":[]}
}

fn indexes_valid(indexes: &[IndexModel]) -> bool {
    indexes.len() == 3
        && indexes.iter().all(|index| {
            index.options.as_ref().is_none_or(|options| {
                options.partial_filter_expression.is_none()
                    && options.sparse != Some(true)
                    && options.hidden != Some(true)
                    && options.collation.is_none()
            })
        })
        && indexes.iter().any(|index| {
            index.keys == doc! {"_id":1}
                && index.options.as_ref().is_some_and(|options| {
                    options.expire_after.is_none() && options.name.as_deref() == Some("_id_")
                })
        })
        && indexes.iter().any(|index| {
            index.keys == doc! {"owner":1,"_id":1}
                && index.options.as_ref().is_some_and(|options| {
                    options.expire_after.is_none()
                        && options.unique != Some(true)
                        && options.name.as_deref() == Some("owner_1__id_1")
                })
        })
        && indexes.iter().any(|index| {
            index.keys == doc! {"expires_at":1}
                && index.options.as_ref().is_some_and(|options| {
                    options.expire_after == Some(Duration::ZERO)
                        && options.unique != Some(true)
                        && options.name.as_deref() == Some("expires_at_1")
                })
        })
}

impl MongoRecords {
    pub(super) async fn media_continuation_heads(
        &self,
        filter: Document,
        owned: bool,
        limit: usize,
    ) -> Result<Vec<RecordHead>, RecordError> {
        let rows: Vec<Head> = self
            .records
            .clone_with_type::<Head>()
            .find(filter)
            .sort(doc! {"_id":1})
            .limit(limit as i64)
            .projection(doc! {"_id":1,"owner":1,"version":1,"schema_version":1})
            .hint(mongodb::options::Hint::Name(
                if owned { "owner_1__id_1" } else { "_id_" }.into(),
            ))
            .max_time(Duration::from_secs(2))
            .await
            .map_err(failure)?
            .try_collect()
            .await
            .map_err(failure)?;
        rows.into_iter()
            .map(|row| {
                if row.version < 1 || row.schema_version != 1 || row.owner.is_empty() {
                    return Err(RecordError::Malformed(
                        "invalid media continuation head".into(),
                    ));
                }
                Ok(RecordHead {
                    record_id: row.id,
                    owner: row.owner,
                    version: row.version as u64,
                })
            })
            .collect()
    }

    pub(crate) async fn media_continuations_ready(&self) -> Result<(), RecordError> {
        let invalid = || RecordError::Unavailable("media continuation index is not ready".into());
        let status = self
            .db
            .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
            .await
            .map_err(failure)?;
        cma_service_auth::mongodb_authority::verify_shared_schema(
            &status,
            self.db.name(),
            &role(self.db.name(), false),
        )
        .map_err(|_| invalid())?;
        if self
            .db
            .collection::<Document>(MIGRATIONS)
            .find_one(doc! {"_id":"media_continuations","version":1})
            .await
            .map_err(failure)?
            .is_none()
        {
            return Err(invalid());
        }
        let indexes: Vec<IndexModel> = self
            .db
            .collection::<Document>(MEDIA_CONTINUATIONS)
            .list_indexes()
            .await
            .map_err(failure)?
            .try_collect()
            .await
            .map_err(failure)?;
        if !indexes_valid(&indexes) {
            return Err(invalid());
        }
        Ok(())
    }
}

pub async fn migrate(db: &Database) -> Result<(), RecordError> {
    cma_service_migrations::marker(db, MIGRATIONS, "media_continuations", "version", &[1], 1)
        .await
        .map_err(failure)?;
    let validator = doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,
    "required":["_id","owner","blob","version","schema_version","expires_at"],"properties":{
    "_id":{"bsonType":"string"},"owner":{"bsonType":"string","minLength":1,"maxLength":512},
    "blob":{"bsonType":"binData"},"version":{"bsonType":"long","minimum":1},
    "schema_version":{"enum":[1]},"guard_revision":{"bsonType":"long","minimum":0},"expires_at":{"bsonType":"date"}}}};
    // The published schema before 1f86a1b6 omitted only the optional guard revision.
    let mut predecessor = validator.clone();
    predecessor
        .get_document_mut("$jsonSchema")
        .unwrap()
        .get_document_mut("properties")
        .unwrap()
        .remove("guard_revision");
    cma_service_migrations::collection_with_predecessors(
        db,
        MEDIA_CONTINUATIONS,
        validator,
        true,
        vec![predecessor],
    )
    .await
    .map_err(failure)?;
    cma_service_migrations::indexes(
        db,
        MEDIA_CONTINUATIONS,
        vec![
            IndexModel::builder().keys(doc! {"owner":1,"_id":1}).build(),
            IndexModel::builder()
                .keys(doc! {"expires_at":1})
                .options(IndexOptions::builder().expire_after(Duration::ZERO).build())
                .build(),
        ],
    )
    .await
    .map_err(failure)?;
    cma_service_migrations::exact_indexes(
        db,
        MEDIA_CONTINUATIONS,
        vec![
            IndexModel::builder().keys(doc! {"owner":1,"_id":1}).build(),
            IndexModel::builder()
                .keys(doc! {"expires_at":1})
                .options(IndexOptions::builder().expire_after(Duration::ZERO).build())
                .build(),
        ],
        indexes_valid,
    )
    .await
    .map_err(failure)?;
    cma_service_migrations::collection_with_predecessors(
        db,
        MIGRATIONS,
        super::schema::migration_validator(),
        false,
        crate::records::mongo::migration_predecessors(),
    )
    .await
    .map_err(failure)?;
    cma_service_migrations::update(
        db,
        MIGRATIONS,
        doc! {"_id":"media_continuations"},
        doc! {"$set":{"version":1}},
        true,
        false,
    )
    .await
    .map_err(failure)?;
    Ok(())
}

#[cfg(test)]
#[path = "mongo_media_continuations_tests.rs"]
mod tests;
