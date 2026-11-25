//! A retained floor in a collection inspected by pre-policy binaries.
use layer2_core::StoreError;
use mongodb::{
    bson::{doc, Document},
    ClientSession, Database,
};

use crate::store::sandbox_mongo::MongoSandboxStore;

pub(in crate::store) const VERSION: i64 = 5;
const COLLECTION: &str = "sandbox_credentials";
const ID: &str = "agent-turn-policy-reader-floor-v1";

fn document() -> Document {
    // Ordinary work already retains the (null, null) credential-index tuple.
    // A negative generation cannot name real credential custody and gives this
    // independent floor a distinct tuple without changing the existing index.
    doc! {"_id":ID,"document_schema_version":VERSION,"protocol":"agent-turn-policy-v1",
    "generation":-1_i64}
}

pub(super) async fn retained(
    store: &MongoSandboxStore,
) -> Result<bool, layer2_core::sandbox::store::SandboxStoreError> {
    let row = store
        .client
        .database(&store.agents.namespace().db)
        .collection::<Document>(COLLECTION)
        .find_one(doc! {"_id":ID})
        .max_time(std::time::Duration::from_secs(1))
        .await
        .map_err(super::unavailable)?;
    match row {
        None => Ok(false),
        Some(row) if row == document() => Ok(true),
        Some(_) => Err(layer2_core::sandbox::store::SandboxStoreError::Corrupt(
            "invalid turn policy reader floor".into(),
        )),
    }
}

pub(in crate::store) async fn retain(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
) -> Result<(), mongodb::error::Error> {
    store
        .client
        .database(&store.agents.namespace().db)
        .collection::<Document>(COLLECTION)
        .update_one(document(), doc! {"$setOnInsert":document()})
        .upsert(true)
        .session(session)
        .await?;
    Ok(())
}

pub(in crate::store) async fn verify(database: &Database) -> Result<(), StoreError> {
    let unavailable = |_| StoreError::Unavailable("turn policy reader floor unavailable".into());
    let malformed =
        || StoreError::Malformed("turn policy reader floor is missing or unrecognized".into());
    let rows = database.collection::<Document>(COLLECTION);
    if rows
        .find_one(doc! {"document_schema_version":VERSION,"_id":{"$ne":ID}})
        .await
        .map_err(unavailable)?
        .is_some()
    {
        return Err(malformed());
    }
    match rows.find_one(doc! {"_id":ID}).await.map_err(unavailable)? {
        Some(row) if row == document() => Ok(()),
        Some(_) => Err(malformed()),
        None => {
            if database
                .collection::<Document>("agents")
                .find_one(doc! {
                    "record.conversation.turn_timeout":{"$exists":true}
                })
                .await
                .map_err(unavailable)?
                .is_some()
            {
                return Err(malformed());
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn turn_policy_floor_is_not_an_ordinary_work_version() {
        let marker = super::document();
        // Credential and Sandbox record schemas have independent version
        // numbers. The aggregate reader still refuses a credential marker.
        assert!(crate::store::work_schema::read::<mongodb::bson::Document>(&marker).is_err());
        assert!(
            marker.get_i64("document_schema_version").unwrap()
                > crate::store::work_schema::PREPARED_VERSION,
            "a rollback reader must reject the retained turn policy marker"
        );
    }
}
