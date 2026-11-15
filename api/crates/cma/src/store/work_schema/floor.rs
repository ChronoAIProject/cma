//! Persistent rollback floor, independent of aggregate and custody cleanup.
use layer2_core::StoreError;
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};

pub(in crate::store) const COLLECTION: &str = "sandbox_credentials";
const SANDBOX_CREDENTIALS: &str = COLLECTION;
const FLOOR: &str = "ordinary-work-writer-floor-v1";

pub(in crate::store) async fn verify(database: &mongodb::Database) -> Result<(), StoreError> {
    let rows = database.collection::<Document>(SANDBOX_CREDENTIALS);
    if rows
        .find_one(doc! {"document_schema_version":3_i64,"_id":{"$ne":FLOOR}})
        .await
        .map_err(|_| StoreError::Unavailable("credential writer floor unavailable".into()))?
        .is_some()
    {
        return Err(StoreError::Malformed(
            "credential writer floor is not recognized".into(),
        ));
    }
    if let Some(row) = rows
        .find_one(doc! {"_id":FLOOR})
        .await
        .map_err(|_| StoreError::Unavailable("credential writer floor unavailable".into()))?
    {
        if row != floor_document() {
            return Err(StoreError::Malformed(
                "credential writer floor is not recognized".into(),
            ));
        }
    } else {
        // Loss or downgrade of the sentinel cannot make existing protected
        // work or an unresolved issuance look like a never-activated database.
        for (name, filter) in [
            ("sandboxes", doc! {"document_schema_version":3_i64}),
            ("workspaces", doc! {"document_schema_version":3_i64}),
            ("sandbox_credential_issuance", doc! {}),
        ] {
            if database
                .collection::<Document>(name)
                .find_one(filter)
                .await
                .map_err(|_| StoreError::Unavailable("work writer floor unavailable".into()))?
                .is_some()
            {
                return Err(StoreError::Malformed(
                    "retained work writer floor is missing".into(),
                ));
            }
        }
    }
    Ok(())
}

fn floor_document() -> Document {
    // This known collection is checked by old binaries at startup. The sentinel
    // survives aggregate deletion, so rolling back cannot erase the writer floor.
    doc! {"_id":FLOOR,"document_schema_version":3_i64,"protocol":"ordinary-work-renewed-v1"}
}

pub(in crate::store) async fn retain(
    store: &crate::store::sandbox_mongo::MongoSandboxStore,
    session: &mut ClientSession,
) -> Result<(), mongodb::error::Error> {
    let rows = store
        .client
        .database(&store.sandboxes.namespace().db)
        .collection::<Document>(COLLECTION);
    retain_rows(&rows, session).await
}

pub(in crate::store) async fn retain_rows(
    rows: &Collection<Document>,
    session: &mut ClientSession,
) -> Result<(), mongodb::error::Error> {
    if rows
        .find_one(floor_document())
        .session(&mut *session)
        .await?
        .is_some()
    {
        return Ok(());
    }
    rows.update_one(floor_document(), doc! {"$setOnInsert":floor_document()})
        .upsert(true)
        .session(session)
        .await?;
    Ok(())
}
