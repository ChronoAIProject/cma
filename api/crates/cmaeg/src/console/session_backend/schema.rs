//! Explicit data-owner migration. Runtime startup only verifies prerequisites;
//! it never creates a collection, changes its validator or creates an index.
use super::{
    mongo::{bounded, majority},
    StoreError, SERVER_TIMEOUT,
};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    options::IndexOptions,
    Database, IndexModel,
};

pub const COLLECTION: &str = "eg_console_sessions";
pub(super) const QUOTA: &str = "quota";

pub fn validator() -> Document {
    let dates = doc! {"bsonType":"date"};
    let session = doc! {"bsonType":"object","additionalProperties":false,
    "required":["_id","kind","subject_hash","payload","created_at","last_seen","observed_at","absolute_at","expires_at","access_expires_at","idle_ms","fence"],
    "properties":{"_id":{"bsonType":"string","minLength":43,"maxLength":64},"kind":{"enum":["session"]},
        "subject_hash":{"bsonType":"string","minLength":43,"maxLength":64},"payload":{"bsonType":"binData"},
        "created_at":dates.clone(),"last_seen":dates.clone(),"observed_at":dates.clone(),"absolute_at":dates.clone(),
        "expires_at":dates.clone(),"access_expires_at":dates.clone(),"idle_ms":{"bsonType":"long","minimum":1},
        "fence":{"bsonType":"long","minimum":0}}};
    let control = doc! {"bsonType":"object","additionalProperties":false,
    "required":["_id","kind","schema_version","fence","observed_at"],
    "properties":{"_id":{"enum":[QUOTA]},"kind":{"enum":["control"]},"schema_version":{"enum":[1]},
        "fence":{"bsonType":"long","minimum":0},"observed_at":dates}};
    doc! {"$jsonSchema":{"oneOf":[session,control]}}
}

pub fn indexes() -> Vec<IndexModel> {
    vec![
        IndexModel::builder()
            .keys(doc! {"expires_at":1})
            .options(
                IndexOptions::builder()
                    .name("console_expiry".to_owned())
                    .expire_after(std::time::Duration::ZERO)
                    .partial_filter_expression(doc! {"kind":"session"})
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! {"kind":1,"subject_hash":1,"created_at":1,"_id":1})
            .options(
                IndexOptions::builder()
                    .name("console_subject_created".to_owned())
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! {"kind":1,"last_seen":1,"_id":1})
            .options(
                IndexOptions::builder()
                    .name("console_activity".to_owned())
                    .build(),
            )
            .build(),
    ]
}

/// Dedicated grants can be combined with the gateway's existing explicit role.
/// No runtime DDL, database-wide data grants or access to another service.
pub fn role(database: &str, migration: bool) -> Document {
    doc! {"createRole":if migration {"cma_eg_console_migrator"} else {"cma_eg_console_runtime"},
    "privileges":[{"resource":{"db":database,"collection":COLLECTION},
        "actions":if migration {vec!["find","insert","update","createCollection","collMod","createIndex","listIndexes"]}
            else {vec!["find","insert","update","remove","listIndexes"]}}],"roles":[]}
}

pub async fn migrate(db: &Database) -> Result<(), StoreError> {
    let concern =
        mongodb::bson::serialize_to_document(&majority()).map_err(|_| StoreError::Invalid)?;
    let result = db.run_command(doc! {"create":COLLECTION,"validator":validator(),
        "validationLevel":"strict","validationAction":"error","writeConcern":concern.clone(),"maxTimeMS":2000_i64}).await;
    match result {
        Ok(_) => {}
        Err(error) if matches!(error.kind.as_ref(),mongodb::error::ErrorKind::Command(command) if command.code == 48) =>
        {
            db.run_command(
                doc! {"collMod":COLLECTION,"validator":validator(),"validationLevel":"strict",
                "validationAction":"error","writeConcern":concern,"maxTimeMS":2000_i64},
            )
            .await
            .map_err(|_| StoreError::Unavailable)?;
        }
        Err(_) => return Err(StoreError::Unavailable),
    }
    let rows = db.collection::<Document>(COLLECTION);
    rows.create_indexes(indexes())
        .write_concern(majority())
        .max_time(SERVER_TIMEOUT)
        .await
        .map_err(|_| StoreError::Unavailable)?;
    rows.update_one(
        doc! {"_id":QUOTA},
        doc! {"$setOnInsert":{"kind":"control","schema_version":1_i32,
        "fence":0_i64,"observed_at":mongodb::bson::DateTime::now()}},
    )
    .upsert(true)
    .write_concern(majority())
    .await
    .map_err(|_| StoreError::Unavailable)?;
    verify(db).await
}

/// Check migration marker and exact required index definitions under the
/// runtime's collection-scoped metadata privileges. Validator DDL is performed
/// only by `migrate`; the schema version is written after those steps succeed.
pub async fn verify(db: &Database) -> Result<(), StoreError> {
    bounded("verify_schema", async {
        let rows = db.collection_with_options::<Document>(
            COLLECTION,
            mongodb::options::CollectionOptions::builder()
                .read_concern(mongodb::options::ReadConcern::majority())
                .selection_criteria(mongodb::options::SelectionCriteria::ReadPreference(
                    mongodb::options::ReadPreference::Primary,
                ))
                .build(),
        );
        if rows
            .find_one(doc! {"_id":QUOTA,"kind":"control","schema_version":1_i32})
            .max_time(SERVER_TIMEOUT)
            .await
            .map_err(|_| StoreError::Unavailable)?
            .is_none()
        {
            return Err(StoreError::Unavailable);
        }
        let actual: Vec<IndexModel> = rows
            .list_indexes()
            .max_time(SERVER_TIMEOUT)
            .await
            .map_err(|_| StoreError::Unavailable)?
            .try_collect()
            .await
            .map_err(|_| StoreError::Unavailable)?;
        if !indexes().iter().all(|required| {
            actual
                .iter()
                .any(|present| index_matches(required, present))
        }) {
            return Err(StoreError::Unavailable);
        }
        Ok(())
    })
    .await
}

fn index_matches(expected: &IndexModel, actual: &IndexModel) -> bool {
    if expected.keys != actual.keys {
        return false;
    }
    let expected = expected.options.clone().unwrap_or_default();
    let actual = actual.options.clone().unwrap_or_default();
    expected.expire_after == actual.expire_after
        && expected.partial_filter_expression == actual.partial_filter_expression
        && !actual.sparse.unwrap_or(false)
        && !actual.unique.unwrap_or(false)
        && !actual.hidden.unwrap_or(false)
        && actual.collation.is_none()
}

/// Check optional runtime privileges only when shared Console mode is enabled.
/// The gateway's existing verifier remains responsible for unrelated grants.
pub async fn verify_grants(db: &Database) -> Result<(), StoreError> {
    bounded("verify_grants", async {
        let status = db
            .run_command(doc! {"connectionStatus":1,"showPrivileges":true,"maxTimeMS":2000_i64})
            .await
            .map_err(|_| StoreError::Unavailable)?;
        runtime_grants(&status, db.name())
    })
    .await
}

fn runtime_grants(status: &Document, database: &str) -> Result<(), StoreError> {
    let entries = status
        .get_document("authInfo")
        .and_then(|info| info.get_array("authenticatedUserPrivileges"))
        .map_err(|_| StoreError::Invalid)?;
    let mut actions = std::collections::BTreeSet::new();
    let allowed = ["find", "insert", "update", "remove", "listIndexes"];
    for entry in entries {
        let entry = entry.as_document().ok_or(StoreError::Invalid)?;
        let resource = entry
            .get_document("resource")
            .map_err(|_| StoreError::Invalid)?;
        if resource.get_str("collection").ok() != Some(COLLECTION) {
            continue;
        }
        if resource.len() != 2 || resource.get_str("db").ok() != Some(database) {
            return Err(StoreError::Invalid);
        }
        for action in entry
            .get_array("actions")
            .map_err(|_| StoreError::Invalid)?
        {
            let action = action.as_str().ok_or(StoreError::Invalid)?;
            if !allowed.contains(&action) {
                return Err(StoreError::Invalid);
            }
            actions.insert(action);
        }
    }
    if allowed.iter().all(|action| actions.contains(action)) {
        Ok(())
    } else {
        Err(StoreError::Invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_privileges_are_exactly_collection_scoped() {
        let role = role("fixture", false);
        let privileges = role.get_array("privileges").unwrap().clone();
        let status = doc! {"authInfo":{"authenticatedUserPrivileges":privileges.clone()}};
        assert_eq!(runtime_grants(&status, "fixture"), Ok(()));
        assert_eq!(runtime_grants(&status, "other"), Err(StoreError::Invalid));
        let mut excess = privileges;
        excess[0]
            .as_document_mut()
            .unwrap()
            .get_array_mut("actions")
            .unwrap()
            .push("collMod".into());
        assert_eq!(
            runtime_grants(
                &doc! {"authInfo":{"authenticatedUserPrivileges":excess}},
                "fixture"
            ),
            Err(StoreError::Invalid)
        );
        assert_eq!(
            runtime_grants(
                &doc! {"authInfo":{"authenticatedUserPrivileges":[]}},
                "fixture"
            ),
            Err(StoreError::Invalid)
        );
    }
}
