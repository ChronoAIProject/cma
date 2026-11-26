use super::{create_fences::CREATE_FENCES, create_fences_floor};
use super::{
    failure, COLLECTIONS, CONNECTIONS, INTERACTIONS, MEDIA_CONTINUATIONS, MIGRATIONS, RECORDS,
    SESSIONS, THREAD_FENCES, WHATSAPP_FLOWS,
};
use crate::records::RecordError;
use crate::worker_credentials::mongo::{CUTOVERS, USES};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    options::IndexOptions,
    Database, IndexModel,
};

// Publication reads this same requirement into the immutable image's OCI label.
// Optional migration markers remain separate from the base startup contract.
pub const BASE_SCHEMA_VERSION: i32 = 2;

pub fn role(database: &str, migration: bool) -> Document {
    let grants:Vec<_>=COLLECTIONS.iter().map(|name|doc!{
        "resource":{"db":database,"collection":*name},
        "actions":if migration {vec!["find","insert","update","createCollection","collMod","createIndex","listIndexes"]} else if *name==MIGRATIONS {vec!["find"]} else {vec!["find","insert","update","remove","listIndexes"]}
    }).collect();
    doc! {"createRole":if migration {"cma_eg_migrator"}else{"cma_eg_runtime"},"privileges":grants,"roles":[]}
}
pub async fn verify(db: &Database) -> Result<(), RecordError> {
    verify_with_authority(
        db,
        cma_service_auth::mongodb_authority::MongoAuthorityMode::Isolated,
    )
    .await
}
pub async fn verify_with_authority(
    db: &Database,
    authority: cma_service_auth::mongodb_authority::MongoAuthorityMode,
) -> Result<(), RecordError> {
    let invalid = || RecordError::Rejected("gateway database credential is not isolated".into());
    let status = db
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
        .await
        .map_err(failure)?;
    if authority == cma_service_auth::mongodb_authority::MongoAuthorityMode::SharedSchema {
        cma_service_auth::mongodb_authority::verify_shared_schema(
            &status,
            db.name(),
            &role(db.name(), false),
        )
        .map_err(|_| invalid())?;
    } else {
        let info = status.get_document("authInfo").map_err(|_| invalid())?;
        if info
            .get_array("authenticatedUsers")
            .map_err(|_| invalid())?
            .len()
            != 1
        {
            return Err(invalid());
        }
        let mut grants =
            std::collections::BTreeMap::<String, std::collections::BTreeSet<String>>::new();
        for grant in info
            .get_array("authenticatedUserPrivileges")
            .map_err(|_| invalid())?
        {
            let grant = grant.as_document().ok_or_else(&invalid)?;
            let resource = grant.get_document("resource").map_err(|_| invalid())?;
            let collection = resource.get_str("collection").map_err(|_| invalid())?;
            if resource.len() != 2
                || resource.get_str("db").ok() != Some(db.name())
                || (!collection.is_empty()
                    && !COLLECTIONS.contains(&collection)
                    && !crate::activity::history::schema::COLLECTIONS.contains(&collection)
                    && ![
                        CREATE_FENCES,
                        THREAD_FENCES,
                        WHATSAPP_FLOWS,
                        "eg_automation_records",
                        MEDIA_CONTINUATIONS,
                        USES,
                        CUTOVERS,
                        crate::console::session_backend::schema::COLLECTION,
                    ]
                    .contains(&collection))
            {
                return Err(invalid());
            }
            for action in grant.get_array("actions").map_err(|_| invalid())? {
                let action = action.as_str().ok_or_else(&invalid)?;
                let allowed = if collection.is_empty() {
                    &["listCollections"][..]
                } else if crate::activity::history::schema::COLLECTIONS.contains(&collection) {
                    crate::activity::history::schema::runtime_actions(collection)
                } else if collection == USES {
                    &["find", "insert", "listIndexes"][..]
                } else if collection == CUTOVERS {
                    &["find", "listIndexes"][..]
                } else if [
                    CREATE_FENCES,
                    THREAD_FENCES,
                    WHATSAPP_FLOWS,
                    "eg_automation_records",
                ]
                .contains(&collection)
                {
                    &["find", "insert", "update", "listIndexes"][..]
                } else if collection != MIGRATIONS {
                    &["find", "insert", "update", "remove", "listIndexes"][..]
                } else {
                    &["find"][..]
                };
                if !allowed.contains(&action) {
                    return Err(invalid());
                }
                grants
                    .entry(collection.into())
                    .or_default()
                    .insert(action.into());
            }
        }
        for collection in COLLECTIONS {
            let required = if *collection == MIGRATIONS {
                &["find"][..]
            } else {
                &["find", "insert", "update", "remove", "listIndexes"][..]
            };
            if !required
                .iter()
                .all(|action| grants.get(*collection).is_some_and(|s| s.contains(*action)))
            {
                return Err(invalid());
            }
        }
    }
    let marker = db
        .collection::<Document>(MIGRATIONS)
        .find_one(doc! {"_id":"schema","version":BASE_SCHEMA_VERSION})
        .await
        .map_err(failure)?;
    if marker.is_none() {
        return Err(RecordError::Unavailable(
            "gateway schema migration required".into(),
        ));
    }
    let indexes: Vec<IndexModel> = db
        .collection::<Document>(INTERACTIONS)
        .list_indexes()
        .await
        .map_err(failure)?
        .try_collect()
        .await
        .map_err(failure)?;
    if !indexes.iter().any(|index| {
        index.keys == doc! {"expires_at":1}
            && index.options.as_ref().is_some_and(|options| {
                options.expire_after == Some(std::time::Duration::ZERO)
                    && options.partial_filter_expression.is_none()
            })
    }) {
        return Err(RecordError::Unavailable(
            "gateway interaction expiry index is required".into(),
        ));
    }
    Ok(())
}

/// Whether the additive channel guard validator/index has been installed.
pub async fn guard_migration_required(db: &Database) -> Result<bool, RecordError> {
    Ok(db
        .collection::<Document>(MIGRATIONS)
        .find_one(doc! {"_id":"github_channel_guard","version":1})
        .await
        .map_err(failure)?
        .is_none())
}
/// Independent optional migrations must preserve every supported marker.
pub(crate) fn migration_validator() -> Document {
    doc! {
        "$jsonSchema":{"bsonType":"object","additionalProperties":false,
            "required":["_id","version"],"properties":{
                "_id":{"bsonType":"string"},"version":{"enum":[1,2,3,4,5,6]}}},
        "$or":[{"_id":"schema","version":BASE_SCHEMA_VERSION},
            {"_id":"replicas","version":1},
            {"_id":"thread_fences","version":1},{"_id":"whatsapp_forms","version":1},
            {"_id":"media_continuations","version":1},{"_id":"worker_credentials","version":1},
            {"_id":"worker_tracking_cutover","version":1},
            {"_id":"github_channel_guard","version":1},
            {"_id":"channel_create_fences","version":1},
            {"_id":create_fences_floor::VALIDATOR_CHANGE,"version":{"$in":[1,2,3]}},
            {"_id":"github_integrations","version":{"$in":[1,2,3,4,5,6]}}]
    }
}
pub(super) fn record_validator(protected: bool) -> Document {
    let mut record_validator = doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":["_id","owner","blob","version","schema_version"],"properties":{
    "_id":{"bsonType":"string"},"owner":{"bsonType":"string","minLength":1,"maxLength":512},"blob":{"bsonType":"binData"},"version":{"bsonType":"long","minimum":1},"schema_version":{"enum":[1]},"guard_revision":{"bsonType":"long","minimum":0}}}};
    record_validator
        .get_document_mut("$jsonSchema")
        .unwrap()
        .get_document_mut("properties")
        .unwrap()
        .insert("replica", super::replica_schema::projection_schema());
    if protected {
        let schema = record_validator.get_document_mut("$jsonSchema").unwrap();
        schema
            .get_array_mut("required")
            .unwrap()
            .push("guard_revision".into());
        schema
            .get_document_mut("properties")
            .unwrap()
            .insert("guard_revision", doc! {"bsonType":"long","minimum":1});
    }
    record_validator
}

pub async fn migrate(db: &Database) -> Result<(), RecordError> {
    cma_service_auth::browser::migrate(db, SESSIONS)
        .await
        .map_err(failure)?;
    let ordinary = record_validator(false);
    let mut interaction_validator = ordinary.clone();
    let interaction_schema = interaction_validator
        .get_document_mut("$jsonSchema")
        .unwrap();
    interaction_schema
        .get_array_mut("required")
        .unwrap()
        .push("expires_at".into());
    interaction_schema
        .get_document_mut("properties")
        .unwrap()
        .insert("expires_at", doc! {"bsonType":"date"});
    for (name, validator) in [
        (MIGRATIONS, migration_validator()),
        (RECORDS, ordinary.clone()),
        (CONNECTIONS, ordinary),
        (INTERACTIONS, interaction_validator),
    ] {
        match db.run_command(doc!{"create":name,"validator":validator.clone(),"validationLevel":"strict","validationAction":"error"}).await{
            Ok(_) => {},
            Err(error) if matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(command) if command.code == 48) => {
                if name == RECORDS {
                    create_fences_floor::upgrade_legacy_validator(db, validator).await?;
                } else {
                    db.run_command(doc! {
                        "collMod": name, "validator": validator,
                        "validationLevel": "strict", "validationAction": "error"
                    }).await.map_err(failure)?;
                }
            },
            Err(error) => return Err(failure(error)),
        }
    }
    for name in [RECORDS, CONNECTIONS, INTERACTIONS] {
        db.collection::<Document>(name)
            .create_index(
                mongodb::IndexModel::builder()
                    .keys(doc! {"owner":1,"_id":1})
                    .build(),
            )
            .await
            .map_err(failure)?;
    }
    db.collection::<Document>(INTERACTIONS)
        .create_index(
            IndexModel::builder()
                .keys(doc! {"expires_at":1})
                .options(
                    IndexOptions::builder()
                        .expire_after(std::time::Duration::ZERO)
                        .build(),
                )
                .build(),
        )
        .await
        .map_err(failure)?;
    db.collection::<Document>(MIGRATIONS)
        .update_one(
            doc! {"_id":"schema"},
            doc! {"$set":{"version":BASE_SCHEMA_VERSION}},
        )
        .upsert(true)
        .await
        .map_err(failure)?;
    db.collection::<Document>(MIGRATIONS)
        .update_one(
            doc! {"_id":"github_channel_guard"},
            doc! {"$set":{"version":1}},
        )
        .upsert(true)
        .await
        .map_err(failure)?;
    Ok(())
}
