//! Explicit collection ownership and a separate migration entry point. The
//! runtime never creates collections, edits validators, or administers roles.

use std::collections::{BTreeMap, BTreeSet};

use mongodb::{
    bson::{doc, Document},
    options::IndexOptions,
    Database, IndexModel,
};

use crate::model::DOCUMENT_VERSION;

pub const SANDBOXES: &str = "sm_sandboxes";
pub const MIGRATIONS: &str = "sm_schema_migrations";
pub const COLLECTIONS: &[&str] = &[SANDBOXES, MIGRATIONS];
const SANDBOX_ACTIONS: &[&str] = &["find", "insert", "update", "listIndexes"];

#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("Sandbox Manager database access is not isolated: {0}")]
    Isolation(&'static str),
    #[error("Sandbox Manager schema migration is required")]
    MigrationRequired,
    #[error("Sandbox Manager schema operation failed")]
    Database(#[from] mongodb::error::Error),
}

/// Exact Mongo privileges for the runtime identity. There is no inherited
/// readWrite role and no collection-prefix wildcard in Mongo's grant syntax.
pub fn runtime_role(database: &str) -> Document {
    doc! {
        "createRole": "cma_sm_runtime",
        "privileges": [
            { "resource": { "db": database, "collection": SANDBOXES }, "actions": SANDBOX_ACTIONS },
            { "resource": { "db": database, "collection": MIGRATIONS }, "actions": ["find"] },
        ],
        "roles": [],
    }
}

pub fn migration_role(database: &str) -> Document {
    let privileges: Vec<_> = COLLECTIONS.iter().map(|name| doc! {
        "resource": { "db": database, "collection": *name },
        "actions": ["find", "insert", "update", "createCollection", "createIndex", "listIndexes", "collMod"],
    }).collect();
    doc! { "createRole": "cma_sm_migrator", "privileges": privileges, "roles": [] }
}

/// Fail closed on inherited/global privileges as well as direct grants. This
/// checks the database's effective authorization, not a URI naming convention.
pub fn validate_runtime_privileges(reply: &Document, database: &str) -> Result<(), SchemaError> {
    let invalid = || SchemaError::Isolation("Mongo did not supply complete authorization evidence");
    let info = reply.get_document("authInfo").map_err(|_| invalid())?;
    if info
        .get_array("authenticatedUsers")
        .map_err(|_| invalid())?
        .len()
        != 1
    {
        return Err(SchemaError::Isolation(
            "exactly one authenticated database identity is required",
        ));
    }
    let privileges = info
        .get_array("authenticatedUserPrivileges")
        .map_err(|_| invalid())?;
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for privilege in privileges {
        let privilege = privilege.as_document().ok_or_else(&invalid)?;
        let resource = privilege.get_document("resource").map_err(|_| invalid())?;
        let name = resource.get_str("collection").map_err(|_| invalid())?;
        if resource.len() != 2
            || resource.get_str("db").map_err(|_| invalid())? != database
            || !COLLECTIONS.contains(&name)
        {
            return Err(SchemaError::Isolation(
                "credential grants access outside this service's collections",
            ));
        }
        let allowed = if name == SANDBOXES {
            SANDBOX_ACTIONS
        } else {
            &["find"]
        };
        for action in privilege.get_array("actions").map_err(|_| invalid())? {
            let action = action.as_str().ok_or_else(&invalid)?;
            if !allowed.contains(&action) {
                return Err(SchemaError::Isolation(
                    "credential grants migration, administration, or unsupported actions",
                ));
            }
            found
                .entry(name.to_owned())
                .or_default()
                .insert(action.to_owned());
        }
    }
    for (name, actions) in [(SANDBOXES, SANDBOX_ACTIONS), (MIGRATIONS, &["find"][..])] {
        if !actions
            .iter()
            .all(|a| found.get(name).is_some_and(|grants| grants.contains(*a)))
        {
            return Err(SchemaError::Isolation(
                "credential is missing required runtime privileges",
            ));
        }
    }
    Ok(())
}

pub async fn verify_runtime(database: &Database) -> Result<(), SchemaError> {
    let status = database
        .run_command(doc! { "connectionStatus": 1, "showPrivileges": true })
        .await?;
    validate_runtime_privileges(&status, database.name())?;
    let marker = database
        .collection::<Document>(MIGRATIONS)
        .find_one(doc! { "_id": "schema" })
        .await?;
    if marker.as_ref().and_then(|m| m.get_i32("version").ok()) != Some(DOCUMENT_VERSION) {
        return Err(SchemaError::MigrationRequired);
    }
    use futures_util::TryStreamExt as _;
    let indexes: Vec<IndexModel> = database
        .collection::<Document>(SANDBOXES)
        .list_indexes()
        .await?
        .try_collect()
        .await?;
    if !indexes.iter().any(|i| {
        i.keys == doc! { "owner": 1, "request_key_hash": 1 }
            && i.options
                .as_ref()
                .is_some_and(|o| o.unique == Some(true) && o.partial_filter_expression.is_none())
    }) {
        return Err(SchemaError::MigrationRequired);
    }
    Ok(())
}

pub fn sandbox_validator() -> Document {
    doc! { "$jsonSchema": {
        "bsonType": "object",
        "additionalProperties": false,
        "required": ["_id", "schema_version", "owner", "request_key_hash", "request_sha256", "spec", "phase", "revision", "generation", "dispatch_id", "runtime", "created_at_ms", "updated_at_ms", "error"],
        "properties": {
            "_id": { "bsonType": "string", "pattern": "^sbx_[0-9a-f]{32}$" },
            "schema_version": { "enum": [DOCUMENT_VERSION] },
            "owner": { "bsonType": "string", "minLength": 1, "maxLength": 512 },
            "request_key_hash": { "bsonType": "string", "pattern": "^[0-9a-f]{64}$" },
            "request_sha256": { "bsonType": "string", "pattern": "^[0-9a-f]{64}$" },
            "spec": {
                "bsonType": "object", "additionalProperties": false,
                "required": ["name", "image", "entrypoint", "resources", "env", "client_ref"],
                "properties": {
                    "name": { "bsonType": "string", "minLength": 1, "maxLength": 128 },
                    "image": { "bsonType": "string", "minLength": 1, "maxLength": 1024 },
                    "entrypoint": { "bsonType": "array", "maxItems": 128, "items": { "bsonType": "string", "maxLength": 8192 } },
                    "resources": {
                        "bsonType": "object", "additionalProperties": false,
                        "required": ["cpu_millis", "memory_mib"],
                        "properties": {
                            "cpu_millis": { "bsonType": ["int", "long"], "minimum": 100, "maximum": 64000 },
                            "memory_mib": { "bsonType": ["int", "long"], "minimum": 128, "maximum": 262144 }
                        }
                    },
                    "env": { "bsonType": "object", "maxProperties": 128, "additionalProperties": { "bsonType": "string", "maxLength": 8192 } },
                    "client_ref": { "bsonType": ["string", "null"], "maxLength": 256 }
                }
            },
            "phase": { "enum": ["accepted", "dispatching", "starting", "ready", "terminating", "terminated", "failed"] },
            "revision": { "bsonType": "long", "minimum": 1 },
            "generation": { "bsonType": "long", "minimum": 1 },
            "dispatch_id": { "bsonType": ["string", "null"] },
            "runtime": {
                "bsonType": ["object", "null"], "additionalProperties": false,
                "required": ["provider_id", "provider_uid", "generation", "image_digest"],
                "properties": {
                    "provider_id": { "bsonType": "string", "minLength": 1, "maxLength": 256 },
                    "provider_uid": { "bsonType": "string", "minLength": 1, "maxLength": 256 },
                    "generation": { "bsonType": "long", "minimum": 1 },
                    "image_digest": { "bsonType": "string", "pattern": "^sha256:[0-9a-f]{64}$" }
                }
            },
            "created_at_ms": { "bsonType": "long", "minimum": 0 },
            "updated_at_ms": { "bsonType": "long", "minimum": 0 },
            "error": { "bsonType": ["string", "null"] },
        }
    } }
}

/// Run with the separate, collection-scoped migrator identity before startup.
pub async fn migrate(database: &Database) -> Result<(), SchemaError> {
    let migrations = database.collection::<Document>(MIGRATIONS);
    if let Some(existing) = migrations.find_one(doc! { "_id": "schema" }).await? {
        if existing.get_i32("version").ok() != Some(DOCUMENT_VERSION) {
            return Err(SchemaError::MigrationRequired);
        }
    }
    let markers = doc! { "$jsonSchema": {
        "bsonType": "object", "additionalProperties": false,
        "required": ["_id", "version"],
        "properties": { "_id": { "enum": ["schema"] }, "version": { "enum": [DOCUMENT_VERSION] } }
    } };
    for (name, validator) in [(SANDBOXES, sandbox_validator()), (MIGRATIONS, markers)] {
        let result = database
            .run_command(doc! {
                "create": name, "validator": &validator,
                "validationLevel": "strict", "validationAction": "error"
            })
            .await;
        match result {
            Ok(_) => {}
            Err(error) if matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(e) if e.code == 48) =>
            {
                database
                    .run_command(doc! { "collMod": name, "validator": validator,
                    "validationLevel": "strict", "validationAction": "error" })
                    .await?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    let collection = database.collection::<Document>(SANDBOXES);
    collection
        .create_indexes([
            IndexModel::builder()
                .keys(doc! { "owner": 1, "request_key_hash": 1 })
                .options(
                    IndexOptions::builder()
                        .name("sm_owner_request".to_owned())
                        .unique(true)
                        .build(),
                )
                .build(),
            IndexModel::builder()
                .keys(doc! { "owner": 1, "_id": 1 })
                .build(),
            IndexModel::builder()
                .keys(doc! { "phase": 1, "updated_at_ms": 1, "_id": 1 })
                .build(),
        ])
        .await?;
    migrations
        .update_one(
            doc! { "_id": "schema" },
            doc! { "$set": { "version": DOCUMENT_VERSION } },
        )
        .upsert(true)
        .await?;
    Ok(())
}
