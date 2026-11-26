//! Persistent rollback floor, independent of aggregate and custody cleanup.
use layer2_core::StoreError;
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};

pub(in crate::store) const COLLECTION: &str = "sandbox_credentials";
const SANDBOX_CREDENTIALS: &str = COLLECTION;
const FLOOR: &str = "ordinary-work-writer-floor-v1";
const RUNTIME_MIGRATION_FLOOR: &str = "runtime-migration-writer-floor-v1";
const RUNTIME_IMAGE_ORIGIN_FLOOR: &str = "runtime-image-origin-writer-floor-v1";
const MANAGED_MACHINE_ORIGIN_FLOOR: &str = "managed-machine-origin-writer-floor-v1";

pub(in crate::store) async fn verify(database: &mongodb::Database) -> Result<(), StoreError> {
    let rows = database.collection::<Document>(SANDBOX_CREDENTIALS);
    if rows
        .find_one(
            doc! {"document_schema_version":3_i64,"_id":{"$nin":[FLOOR,RUNTIME_MIGRATION_FLOOR,RUNTIME_IMAGE_ORIGIN_FLOOR,MANAGED_MACHINE_ORIGIN_FLOOR]}},
        )
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
    let migration_floor = rows
        .find_one(doc! {"_id":RUNTIME_MIGRATION_FLOOR})
        .await
        .map_err(|_| {
            StoreError::Unavailable("runtime migration writer floor unavailable".into())
        })?;
    match migration_floor {
        Some(row) if row == runtime_migration_floor_document() => {}
        Some(_) => {
            return Err(StoreError::Malformed(
                "runtime migration writer floor is not recognized".into(),
            ))
        }
        None => {
            if database
                .collection::<Document>("sandboxes")
                .find_one(doc! {"document_schema_version":super::RUNTIME_MIGRATION_VERSION})
                .await
                .map_err(|_| {
                    StoreError::Unavailable("runtime migration writer floor unavailable".into())
                })?
                .is_some()
            {
                return Err(StoreError::Malformed(
                    "runtime migration writer floor is missing".into(),
                ));
            }
        }
    }
    for (id, envelope, expected) in [
        (
            RUNTIME_IMAGE_ORIGIN_FLOOR,
            super::RUNTIME_IMAGE_ORIGIN_VERSION,
            runtime_image_origin_floor_document(),
        ),
        (
            MANAGED_MACHINE_ORIGIN_FLOOR,
            super::MANAGED_MACHINE_ORIGIN_VERSION,
            managed_machine_origin_floor_document(),
        ),
    ] {
        verify_image_floor(database, &rows, id, envelope, expected).await?;
    }
    Ok(())
}

async fn verify_image_floor(
    database: &mongodb::Database,
    rows: &Collection<Document>,
    id: &str,
    envelope: i64,
    expected: Document,
) -> Result<(), StoreError> {
    match rows
        .find_one(doc! {"_id": id})
        .await
        .map_err(|_| image_origin_unavailable())?
    {
        Some(row) if row == expected => Ok(()),
        Some(_) => Err(StoreError::Malformed(format!("{id} is not recognized"))),
        None => {
            let protected = database
                .collection::<Document>("sandboxes")
                .find_one(doc! {"document_schema_version": envelope})
                .await
                .map_err(|_| image_origin_unavailable())?;
            if protected.is_some() {
                return Err(StoreError::Malformed(format!("{id} is missing")));
            }
            Ok(())
        }
    }
}

fn image_origin_unavailable() -> StoreError {
    StoreError::Unavailable("runtime image origin writer floor unavailable".into())
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

fn runtime_migration_floor_document() -> Document {
    // A separate index key lets this floor coexist with the ordinary-work
    // marker (no generation) and the turn-timeout marker (generation -1).
    // The v3 marker still fences rollback after the last migrated Sandbox retires.
    doc! {"_id":RUNTIME_MIGRATION_FLOOR,"document_schema_version":3_i64,"protocol":"custom-runtime-authority-v1",
    "generation":-2_i64}
}

pub(in crate::store) async fn retain_runtime_migration(
    store: &crate::store::sandbox_mongo::MongoSandboxStore,
    session: &mut ClientSession,
) -> Result<(), mongodb::error::Error> {
    let rows = store
        .client
        .database(&store.sandboxes.namespace().db)
        .collection::<Document>(COLLECTION);
    rows.update_one(
        runtime_migration_floor_document(),
        doc! {"$setOnInsert":runtime_migration_floor_document()},
    )
    .upsert(true)
    .session(session)
    .await?;
    Ok(())
}

fn runtime_image_origin_floor_document() -> Document {
    doc! {"_id":RUNTIME_IMAGE_ORIGIN_FLOOR,"document_schema_version":3_i64,
    "protocol":"runtime-image-origin-v1","generation":-3_i64}
}

fn managed_machine_origin_floor_document() -> Document {
    doc! {"_id":MANAGED_MACHINE_ORIGIN_FLOOR,"document_schema_version":3_i64,
    "protocol":"managed-machine-origin-v1","generation":-4_i64}
}

pub(in crate::store) async fn retain_runtime_image_origin(
    store: &crate::store::sandbox_mongo::MongoSandboxStore,
    session: &mut ClientSession,
) -> Result<(), mongodb::error::Error> {
    retain_image_floor(store, session, runtime_image_origin_floor_document()).await
}

pub(in crate::store) async fn retain_managed_machine_origin(
    store: &crate::store::sandbox_mongo::MongoSandboxStore,
    session: &mut ClientSession,
) -> Result<(), mongodb::error::Error> {
    retain_image_floor(store, session, managed_machine_origin_floor_document()).await
}

async fn retain_image_floor(
    store: &crate::store::sandbox_mongo::MongoSandboxStore,
    session: &mut ClientSession,
    floor: Document,
) -> Result<(), mongodb::error::Error> {
    let rows = store
        .client
        .database(&store.sandboxes.namespace().db)
        .collection::<Document>(COLLECTION);
    if rows
        .find_one(floor.clone())
        .session(&mut *session)
        .await?
        .is_none()
    {
        rows.update_one(floor.clone(), doc! {"$setOnInsert":floor})
            .upsert(true)
            .session(session)
            .await?;
    }
    Ok(())
}
