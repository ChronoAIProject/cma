//! Image Manager's exclusive schema and effective-grant verification.
use crate::model::DOCUMENT_VERSION;
use mongodb::{
    bson::{doc, Document},
    options::IndexOptions,
    Database, IndexModel,
};
use std::collections::{BTreeMap, BTreeSet};

pub const IMAGES: &str = "im_images";
pub const BUILDS: &str = "im_builds";
pub const MIGRATIONS: &str = "im_schema_migrations";
pub const SESSIONS: &str = "im_auth_sessions";
pub const COLLECTIONS: &[&str] = &[IMAGES, BUILDS, MIGRATIONS, SESSIONS];
// Collection rules evolve separately from backward-compatible document fields.
const SCHEMA_VERSION: i32 = 2;
const SESSION_ACTIONS: &[&str] = &["find", "insert", "update", "remove", "listIndexes"];
const ACTIONS: &[&str] = &["find", "insert", "update", "listIndexes"];

#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("Image Manager database credential is not isolated")]
    Isolation,
    #[error("Image Manager database migration is required")]
    MigrationRequired,
    #[error("Image Manager schema operation failed")]
    Database(#[from] mongodb::error::Error),
}

pub fn runtime_role(database: &str) -> Document {
    let grants: Vec<_> = COLLECTIONS
        .iter()
        .map(|name| {
            doc! {
                "resource":{"db":database,"collection":*name},
                "actions":if *name==MIGRATIONS {&["find"][..]} else if *name==SESSIONS {SESSION_ACTIONS} else {ACTIONS}
            }
        })
        .collect();
    doc! {"createRole":"cma_im_runtime","privileges":grants,"roles":[]}
}
pub fn migration_role(database: &str) -> Document {
    let grants: Vec<_> = COLLECTIONS.iter().map(|name| doc! {
        "resource":{"db":database,"collection":*name},
        "actions":["find","insert","update","createCollection","createIndex","listIndexes","collMod"]
    }).collect();
    cma_service_migrations::migration_role(
        doc! {"createRole":"cma_im_migrator","privileges":grants,"roles":[]},
        database,
        crate::migrations::LEDGER,
    )
}
pub fn validate_runtime_privileges(reply: &Document, database: &str) -> Result<(), SchemaError> {
    let info = reply
        .get_document("authInfo")
        .map_err(|_| SchemaError::Isolation)?;
    if info
        .get_array("authenticatedUsers")
        .map_err(|_| SchemaError::Isolation)?
        .len()
        != 1
    {
        return Err(SchemaError::Isolation);
    }
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for entry in info
        .get_array("authenticatedUserPrivileges")
        .map_err(|_| SchemaError::Isolation)?
    {
        let entry = entry.as_document().ok_or(SchemaError::Isolation)?;
        let resource = entry
            .get_document("resource")
            .map_err(|_| SchemaError::Isolation)?;
        let collection = resource
            .get_str("collection")
            .map_err(|_| SchemaError::Isolation)?;
        if resource.len() != 2
            || resource.get_str("db").ok() != Some(database)
            || !COLLECTIONS.contains(&collection)
        {
            return Err(SchemaError::Isolation);
        }
        let allowed = if collection == MIGRATIONS {
            &["find"][..]
        } else if collection == SESSIONS {
            SESSION_ACTIONS
        } else {
            ACTIONS
        };
        for action in entry
            .get_array("actions")
            .map_err(|_| SchemaError::Isolation)?
        {
            let action = action.as_str().ok_or(SchemaError::Isolation)?;
            if !allowed.contains(&action) {
                return Err(SchemaError::Isolation);
            }
            found
                .entry(collection.into())
                .or_default()
                .insert(action.into());
        }
    }
    for collection in COLLECTIONS {
        let required = if *collection == MIGRATIONS {
            &["find"][..]
        } else if *collection == SESSIONS {
            SESSION_ACTIONS
        } else {
            ACTIONS
        };
        if !required
            .iter()
            .all(|a| found.get(*collection).is_some_and(|s| s.contains(*a)))
        {
            return Err(SchemaError::Isolation);
        }
    }
    Ok(())
}

pub async fn verify_runtime(db: &Database) -> Result<(), SchemaError> {
    verify_runtime_with_authority(
        db,
        cma_service_auth::mongodb_authority::MongoAuthorityMode::Isolated,
    )
    .await
}
pub async fn verify_runtime_with_authority(
    db: &Database,
    authority: cma_service_auth::mongodb_authority::MongoAuthorityMode,
) -> Result<(), SchemaError> {
    let status = db
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
        .await?;
    match authority {
        cma_service_auth::mongodb_authority::MongoAuthorityMode::Isolated => {
            validate_runtime_privileges(&status, db.name())?
        }
        cma_service_auth::mongodb_authority::MongoAuthorityMode::SharedSchema => {
            cma_service_auth::mongodb_authority::verify_shared_schema(
                &status,
                db.name(),
                &runtime_role(db.name()),
            )
            .map_err(|_| SchemaError::Isolation)?;
        }
    }
    let marker = db
        .collection::<Document>(MIGRATIONS)
        .find_one(doc! {"_id":"schema"})
        .await?;
    if marker.as_ref().and_then(|d| d.get_i32("version").ok()) != Some(SCHEMA_VERSION) {
        return Err(SchemaError::MigrationRequired);
    }
    use futures_util::TryStreamExt;
    for name in [IMAGES, BUILDS] {
        let indexes: Vec<IndexModel> = db
            .collection::<Document>(name)
            .list_indexes()
            .await?
            .try_collect()
            .await?;
        if !indexes.iter().any(|i| {
            i.keys == doc! {"owner":1,"request_key_hash":1}
                && i.options.as_ref().is_some_and(|o| {
                    o.unique == Some(true) && o.partial_filter_expression.is_none()
                })
        }) {
            return Err(SchemaError::MigrationRequired);
        }
    }
    Ok(())
}

pub fn validator(build: bool) -> Document {
    let mut properties = doc! {
        "_id":{"bsonType":"string","pattern":if build {"^bld_[0-9a-f]{32}$"} else {"^img_[0-9a-f]{32}$"}},
        "schema_version":{"enum":[DOCUMENT_VERSION]},"owner":{"bsonType":"string","minLength":1,"maxLength":512},
        "request_key_hash":{"bsonType":"string","pattern":"^[0-9a-f]{64}$"},"request_sha256":{"bsonType":"string","pattern":"^[0-9a-f]{64}$"},
        "revision":{"bsonType":"long","minimum":1},"created_at_ms":{"bsonType":"long"}
    };
    if build {
        properties.extend(doc! {
            "image_id":{"bsonType":"string","pattern":"^img_[0-9a-f]{32}$"},
            "source":{"bsonType":"object","additionalProperties":false,"required":["repository","commit","dockerfile","context"],"properties":{
                "repository":{"bsonType":"string","maxLength":1024},"commit":{"bsonType":"string","pattern":"^[0-9a-f]{40}$"},
                "dockerfile":{"bsonType":"string","maxLength":512},"context":{"bsonType":"string","maxLength":512},
                "dockerfile_content":{"bsonType":"string","maxLength":16384}}},
            "phase":{"enum":["queued","dispatching","running","succeeded","failed","cancelled"]},
            "cloud_id":{"bsonType":["string","null"]},"digest":{"bsonType":["string","null"],"pattern":"^sha256:[0-9a-f]{64}$"},
            "error":{"bsonType":["string","null"],"maxLength":2048},"updated_at_ms":{"bsonType":"long"}
        });
    } else {
        properties.extend(doc! {
            "spec":{"bsonType":"object","additionalProperties":false,"required":["name","description"],"properties":{
                "name":{"bsonType":"string","minLength":1,"maxLength":128},"description":{"bsonType":"string","maxLength":4096}}},
            "shares":{"bsonType":"array","maxItems":1000,"uniqueItems":true,"items":{"bsonType":"string","minLength":1,"maxLength":512}},
            "retired":{"bsonType":"bool"},
            "visibility":{"enum":["private","public"]}
        });
    }
    let required: Vec<_> = properties
        .keys()
        .filter(|key| key.as_str() != "visibility")
        .cloned()
        .collect();
    doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":required,"properties":properties}}
}

pub async fn migrate(db: &Database) -> Result<(), SchemaError> {
    cma_service_migrations::marker_int32(
        db,
        MIGRATIONS,
        "schema",
        "version",
        &[1, 2],
        SCHEMA_VERSION,
    )
    .await?;
    cma_service_auth::browser::migrate(db, SESSIONS).await?;
    for name in [IMAGES, BUILDS, MIGRATIONS].iter() {
        let validation = if *name == MIGRATIONS {
            doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":["_id","version"],"properties":{"_id":{"enum":["schema"]},"version":{"enum":[SCHEMA_VERSION]}}}}
        } else {
            validator(*name == BUILDS)
        };
        let predecessors = if *name == MIGRATIONS {
            vec![
                doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":["_id","version"],"properties":{"_id":{"enum":["schema"]},"version":{"enum":[1]}}}},
            ]
        } else {
            let mut prior = validation.clone();
            let properties = prior
                .get_document_mut("$jsonSchema")
                .unwrap()
                .get_document_mut("properties")
                .unwrap();
            if *name == IMAGES {
                properties.remove("visibility");
            } else {
                properties
                    .get_document_mut("source")
                    .unwrap()
                    .get_document_mut("properties")
                    .unwrap()
                    .remove("dockerfile_content");
            }
            vec![prior]
        };
        cma_service_migrations::collection_with_predecessors(
            db,
            name,
            validation,
            true,
            predecessors,
        )
        .await?;
    }
    for name in [IMAGES, BUILDS] {
        cma_service_migrations::indexes(
            db,
            name,
            vec![
                IndexModel::builder()
                    .keys(doc! {"owner":1,"request_key_hash":1})
                    .options(IndexOptions::builder().unique(true).build())
                    .build(),
                IndexModel::builder().keys(doc! {"owner":1,"_id":1}).build(),
            ],
        )
        .await?;
    }
    cma_service_migrations::indexes(
        db,
        BUILDS,
        vec![
            IndexModel::builder()
                .keys(doc! {"phase":1,"updated_at_ms":1})
                .build(),
            IndexModel::builder()
                .keys(doc! {"image_id":1,"_id":1})
                .build(),
        ],
    )
    .await?;
    cma_service_migrations::indexes(
        db,
        IMAGES,
        vec![
            IndexModel::builder()
                .keys(doc! {"visibility":1,"retired":1,"_id":1})
                .build(),
            IndexModel::builder()
                .keys(doc! {"shares":1,"_id":1})
                .build(),
        ],
    )
    .await?;
    cma_service_migrations::update(
        db,
        MIGRATIONS,
        doc! {"_id":"schema"},
        doc! {"$set":{"version":SCHEMA_VERSION}},
        true,
        false,
    )
    .await?;
    Ok(())
}
