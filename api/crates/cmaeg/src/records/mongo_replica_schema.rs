//! Additive discovery indexes; installation does not enable replica writers.
use super::{failure, migration_validator, RecordError, CONNECTIONS, INTERACTIONS, MIGRATIONS};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    options::IndexOptions,
    Database, IndexModel,
};

const INDEX: &str = "replica_active_discovery_v1";

pub(super) fn projection_schema() -> Document {
    doc! {"bsonType":"object","additionalProperties":false,
    "required":["kind","deployment","active"],"properties":{
        "kind":{"enum":["member","partition","effect","admission"]},
        "deployment":{"bsonType":"string","pattern":"^[0-9a-f]{64}$"},
        "active":{"bsonType":"bool"}
    }}
}

fn keys() -> Document {
    doc! {"replica.kind":1,"replica.deployment":1,"replica.active":1,"_id":1}
}

pub async fn migrate(db: &Database) -> Result<(), RecordError> {
    super::migrate(db).await?;
    for collection in [CONNECTIONS, INTERACTIONS] {
        db.collection::<Document>(collection)
            .create_index(
                IndexModel::builder()
                    .keys(keys())
                    .options(
                        IndexOptions::builder()
                            .name(INDEX.to_owned())
                            .partial_filter_expression(doc! {"replica":{"$exists":true}})
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(failure)?;
    }
    db.run_command(doc! {"collMod":MIGRATIONS,"validator":migration_validator()})
        .await
        .map_err(failure)?;
    db.collection::<Document>(MIGRATIONS)
        .update_one(doc! {"_id":"replicas"}, doc! {"$set":{"version":1}})
        .upsert(true)
        .await
        .map_err(failure)?;
    verify(db).await
}

pub(crate) async fn verify(db: &Database) -> Result<(), RecordError> {
    if db
        .collection::<Document>(MIGRATIONS)
        .find_one(doc! {"_id":"replicas","version":1})
        .await
        .map_err(failure)?
        .is_none()
    {
        return Err(RecordError::Rejected(
            "replica journal migration is required".into(),
        ));
    }
    for collection in [CONNECTIONS, INTERACTIONS] {
        let indexes: Vec<IndexModel> = db
            .collection::<Document>(collection)
            .list_indexes()
            .await
            .map_err(failure)?
            .try_collect()
            .await
            .map_err(failure)?;
        if !indexes.iter().any(|index| {
            index.keys == keys()
                && index.options.as_ref().is_some_and(|options| {
                    options.name.as_deref() == Some(INDEX)
                        && options.partial_filter_expression
                            == Some(doc! {"replica":{"$exists":true}})
                        && options.expire_after.is_none()
                })
        }) {
            return Err(RecordError::Rejected(
                "replica journal discovery index is required".into(),
            ));
        }
    }
    Ok(())
}
