use super::{
    failure, COLLECTIONS, CONNECTIONS, INTERACTIONS, MIGRATIONS, RECORDS, SESSIONS, THREAD_FENCES,
};
use crate::records::RecordError;
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    options::IndexOptions,
    Database, IndexModel,
};

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
                || (!COLLECTIONS.contains(&collection) && collection != THREAD_FENCES)
            {
                return Err(invalid());
            }
            for action in grant.get_array("actions").map_err(|_| invalid())? {
                let action = action.as_str().ok_or_else(&invalid)?;
                let allowed = if collection == THREAD_FENCES {
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
        .find_one(doc! {"_id":"schema","version":2})
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
/// Independent optional migrations must preserve every supported marker.
pub(super) fn migration_validator() -> Document {
    doc! {
        "$jsonSchema":{"bsonType":"object","additionalProperties":false,
            "required":["_id","version"],"properties":{
                "_id":{"bsonType":"string"},"version":{"enum":[1,2]}}},
        "$or":[{"_id":"schema","version":2},
            {"_id":"thread_fences","version":1},{"_id":"whatsapp_forms","version":1}]
    }
}
pub async fn migrate(db: &Database) -> Result<(), RecordError> {
    cma_service_auth::browser::migrate(db, SESSIONS)
        .await
        .map_err(failure)?;
    let record_validator = doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":["_id","owner","blob","version","schema_version"],"properties":{
    "_id":{"bsonType":"string"},"owner":{"bsonType":"string","minLength":1,"maxLength":512},"blob":{"bsonType":"binData"},"version":{"bsonType":"long","minimum":1},"schema_version":{"enum":[1]}}}};
    let mut interaction_validator = record_validator.clone();
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
        (RECORDS, record_validator.clone()),
        (CONNECTIONS, record_validator),
        (INTERACTIONS, interaction_validator),
        (MIGRATIONS, migration_validator()),
    ] {
        match db.run_command(doc!{"create":name,"validator":validator.clone(),"validationLevel":"strict","validationAction":"error"}).await{
            Ok(_)=>{},Err(error) if matches!(error.kind.as_ref(),mongodb::error::ErrorKind::Command(c) if c.code==48)=>{db.run_command(doc!{"collMod":name,"validator":validator,"validationLevel":"strict","validationAction":"error"}).await.map_err(failure)?;},Err(error)=>return Err(failure(error)),
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
        .update_one(doc! {"_id":"schema"}, doc! {"$set":{"version":2}})
        .upsert(true)
        .await
        .map_err(failure)?;
    Ok(())
}
