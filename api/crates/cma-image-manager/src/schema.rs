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
    doc! {"createRole":"cma_im_migrator","privileges":grants,"roles":[]}
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
    validate_runtime_privileges(
        &db.run_command(doc! {"connectionStatus":1,"showPrivileges":true})
            .await?,
        db.name(),
    )?;
    let marker = db
        .collection::<Document>(MIGRATIONS)
        .find_one(doc! {"_id":"schema"})
        .await?;
    if marker.as_ref().and_then(|d| d.get_i32("version").ok()) != Some(DOCUMENT_VERSION) {
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
                "dockerfile":{"bsonType":"string","maxLength":512},"context":{"bsonType":"string","maxLength":512}}},
            "phase":{"enum":["queued","dispatching","running","succeeded","failed","cancelled"]},
            "cloud_id":{"bsonType":["string","null"]},"digest":{"bsonType":["string","null"],"pattern":"^sha256:[0-9a-f]{64}$"},
            "error":{"bsonType":["string","null"],"maxLength":2048},"updated_at_ms":{"bsonType":"long"}
        });
    } else {
        properties.extend(doc! {
            "spec":{"bsonType":"object","additionalProperties":false,"required":["name","description"],"properties":{
                "name":{"bsonType":"string","minLength":1,"maxLength":128},"description":{"bsonType":"string","maxLength":4096}}},
            "shares":{"bsonType":"array","maxItems":1000,"uniqueItems":true,"items":{"bsonType":"string","minLength":1,"maxLength":512}},
            "retired":{"bsonType":"bool"}
        });
    }
    let required: Vec<_> = properties.keys().cloned().collect();
    doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":required,"properties":properties}}
}

pub async fn migrate(db: &Database) -> Result<(), SchemaError> {
    cma_service_auth::browser::migrate(db, SESSIONS).await?;
    for name in [IMAGES, BUILDS, MIGRATIONS].iter() {
        let validation = if *name == MIGRATIONS {
            doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":["_id","version"],"properties":{"_id":{"enum":["schema"]},"version":{"enum":[DOCUMENT_VERSION]}}}}
        } else {
            validator(*name == BUILDS)
        };
        match db.run_command(doc!{"create":*name,"validator":validation.clone(),"validationLevel":"strict","validationAction":"error"}).await {
            Ok(_) => {},
            Err(error) if matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(c) if c.code==48) => {
                db.run_command(doc!{"collMod":*name,"validator":validation,"validationLevel":"strict","validationAction":"error"}).await?;
            },
            Err(error) => return Err(error.into()),
        }
    }
    for name in [IMAGES, BUILDS] {
        let collection = db.collection::<Document>(name);
        collection
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"owner":1,"request_key_hash":1})
                    .options(IndexOptions::builder().unique(true).build())
                    .build(),
            )
            .await?;
        collection
            .create_index(IndexModel::builder().keys(doc! {"owner":1,"_id":1}).build())
            .await?;
    }
    db.collection::<Document>(BUILDS)
        .create_index(
            IndexModel::builder()
                .keys(doc! {"phase":1,"updated_at_ms":1})
                .build(),
        )
        .await?;
    db.collection::<Document>(BUILDS)
        .create_index(
            IndexModel::builder()
                .keys(doc! {"image_id":1,"_id":1})
                .build(),
        )
        .await?;
    db.collection::<Document>(MIGRATIONS)
        .update_one(
            doc! {"_id":"schema"},
            doc! {"$set":{"version":DOCUMENT_VERSION}},
        )
        .upsert(true)
        .await?;
    Ok(())
}
