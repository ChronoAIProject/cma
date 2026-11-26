//! Explicit additive migration; runtime can verify but cannot install schema.
use super::{repository::CaptureBoundary, Error};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    options::IndexOptions,
    Database, IndexModel,
};

pub const EVENTS: &str = "eg_activity_history_v1";
pub const CHUNKS: &str = "eg_activity_history_chunks_v1";
pub const SCOPES: &str = "eg_activity_history_scopes_v1";
pub const CONTROL: &str = "eg_activity_history_control_v1";
pub const COLLECTIONS: &[&str] = &[EVENTS, CHUNKS, SCOPES, CONTROL];
const TTL: &str = "activity_history_expiry";
pub(super) const PAGE_INDEX: &str = "activity_history_scope_order";

pub fn role(database: &str, migration: bool) -> Document {
    let privileges: Vec<_> = COLLECTIONS
        .iter()
        .map(|collection| {
            let actions = if migration {
                vec![
                    "find",
                    "insert",
                    "update",
                    "createCollection",
                    "collMod",
                    "createIndex",
                    "listIndexes",
                ]
            } else if *collection == CONTROL {
                vec!["find", "listIndexes"]
            } else if *collection == CHUNKS {
                vec!["find", "insert", "listIndexes"]
            } else {
                vec!["find", "insert", "update", "listIndexes"]
            };
            doc! {"resource":{"db":database,"collection":*collection},"actions":actions}
        })
        .collect();
    doc! {"createRole":if migration {"cma_eg_activity_history_migrator"} else {"cma_eg_activity_history_runtime"},"privileges":privileges,"roles":[]}
}
fn validator(name: &str) -> Document {
    let digest = doc! {"bsonType":"string","pattern":"^[0-9a-f]{64}$"};
    let long = doc! {"bsonType":"long","minimum":1};
    let date = doc! {"bsonType":"date"};
    let mut fields = doc! {"_id":digest.clone()};
    match name {
        EVENTS => fields.extend(doc! {"scope":digest.clone(),"received_at":date.clone(),"sequence":long.clone(),"version":long.clone(),"expires_at":date,"blob":{"bsonType":"binData"}}),
        CHUNKS => fields.extend(doc! {"scope":digest.clone(),"event_key":digest,"expires_at":date,"blob":{"bsonType":"binData"}}),
        SCOPES => fields.extend(doc! {"sequence":long}),
        CONTROL => fields = doc! {"_id":{"enum":["capture"]},"schema_version":{"enum":[1]},"revision":long,"retention_days":{"bsonType":"int","minimum":1,"maximum":90},"since_ms":{"bsonType":["long","null"]},"until_ms":{"bsonType":["long","null"]}},
        _ => unreachable!(),
    }
    let required: Vec<_> = fields.keys().cloned().collect();
    doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":required,"properties":fields}}
}
pub(super) fn indexes(name: &str) -> Vec<IndexModel> {
    let mut indexes = Vec::new();
    if [EVENTS, CHUNKS].contains(&name) {
        indexes.push(
            IndexModel::builder()
                .keys(doc! {"expires_at":1})
                .options(
                    IndexOptions::builder()
                        .name(TTL.to_owned())
                        .expire_after(std::time::Duration::ZERO)
                        .build(),
                )
                .build(),
        );
    }
    if name == EVENTS {
        indexes.push(
            IndexModel::builder()
                .keys(doc! {"scope":1,"received_at":-1,"_id":1})
                .options(IndexOptions::builder().name(PAGE_INDEX.to_owned()).build())
                .build(),
        );
        indexes.push(
            IndexModel::builder()
                .keys(doc! {"scope":1,"expires_at":1})
                .options(
                    IndexOptions::builder()
                        .name("activity_history_scope_expiry".to_owned())
                        .build(),
                )
                .build(),
        );
    }
    indexes
}
pub async fn migrate(db: &Database) -> Result<(), Error> {
    cma_service_migrations::marker_int32(db, CONTROL, "capture", "schema_version", &[1], 1)
        .await
        .map_err(|_| Error::InvalidConfiguration)?;
    for name in COLLECTIONS {
        cma_service_migrations::collection(db, name, validator(name), true)
            .await
            .map_err(|_| Error::Unavailable)?;
        cma_service_migrations::indexes(db, name, indexes(name))
            .await
            .map_err(|_| Error::Unavailable)?;
    }
    cma_service_migrations::update(db, CONTROL, doc! {"_id":"capture"}, doc! {"$setOnInsert":{"schema_version":1_i32,"revision":1_i64,"retention_days":7_i32,"since_ms":null,"until_ms":null}}, true, false).await.map_err(|_| Error::Unavailable)?;
    if cma_service_migrations::inspecting() {
        return Ok(());
    }
    verify(db).await
}
pub async fn verify(db: &Database) -> Result<(), Error> {
    let marker = db
        .collection::<Document>(CONTROL)
        .find_one(doc! {"_id":"capture","schema_version":1_i32})
        .await
        .map_err(|_| Error::Unavailable)?;
    if marker.is_none() {
        return Err(Error::Unavailable);
    }
    for name in COLLECTIONS {
        let actual: Vec<IndexModel> = db
            .collection::<Document>(name)
            .list_indexes()
            .await
            .map_err(|_| Error::Unavailable)?
            .try_collect()
            .await
            .map_err(|_| Error::Unavailable)?;
        for required in indexes(name) {
            if !actual.iter().any(|index| index_matches(&required, index)) {
                return Err(Error::Unavailable);
            }
        }
    }
    Ok(())
}
fn index_matches(expected: &IndexModel, actual: &IndexModel) -> bool {
    let e = expected.options.clone().unwrap_or_default();
    let a = actual.options.clone().unwrap_or_default();
    expected.keys == actual.keys
        && e.name == a.name
        && e.expire_after == a.expire_after
        && a.partial_filter_expression.is_none()
        && !a.hidden.unwrap_or(false)
        && !a.sparse.unwrap_or(false)
        && !a.unique.unwrap_or(false)
        && a.collation.is_none()
}
/// Only the separately privileged release/migration identity can attest that
/// all active and draining writers captured receipts, or close that guarantee
/// before rollback. Runtime must never self-attest completeness.
pub async fn attest(
    db: &Database,
    boundary: CaptureBoundary,
    expected_revision: i64,
) -> Result<(), Error> {
    if !(1..=90).contains(&boundary.retention_days)
        || boundary.revision
            != expected_revision
                .checked_add(1)
                .ok_or(Error::InvalidRequest)?
        || boundary
            .since_ms
            .is_some_and(|s| s <= 0 || s > super::now_ms())
        || boundary
            .until_ms
            .is_some_and(|u| boundary.since_ms.is_none_or(|s| u < s) || u > super::now_ms())
    {
        return Err(Error::InvalidRequest);
    }
    let result = db.collection::<Document>(CONTROL).update_one(doc! {"_id":"capture","revision":expected_revision},doc! {"$set":{
        "revision":boundary.revision,"retention_days":i32::from(boundary.retention_days),"since_ms":boundary.since_ms,"until_ms":boundary.until_ms}})
        .write_concern(super::mongo::majority()).await.map_err(|_| Error::Unavailable)?;
    if result.matched_count != 1 {
        return Err(Error::InvalidRequest);
    }
    Ok(())
}

pub(crate) fn runtime_actions(collection: &str) -> &'static [&'static str] {
    match collection {
        CONTROL => &["find", "listIndexes"],
        CHUNKS => &["find", "insert", "listIndexes"],
        _ => &["find", "insert", "update", "listIndexes"],
    }
}
pub async fn verify_grants(db: &Database) -> Result<(), Error> {
    use cma_service_auth::mongodb_authority::{verify_shared_schema, MongoAuthorityMode};
    let status = db
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
        .await
        .map_err(|_| Error::Unavailable)?;
    let mode = MongoAuthorityMode::from_env(crate::CMAEG_MONGODB_AUTHORITY_MODE)
        .map_err(|_| Error::InvalidConfiguration)?;
    if mode == MongoAuthorityMode::SharedSchema {
        return verify_shared_schema(&status, db.name(), &role(db.name(), false))
            .map_err(|_| Error::InvalidConfiguration);
    }
    let grants = status
        .get_document("authInfo")
        .and_then(|i| i.get_array("authenticatedUserPrivileges"))
        .map_err(|_| Error::InvalidConfiguration)?;
    for collection in COLLECTIONS {
        let mut actual = std::collections::BTreeSet::new();
        for grant in grants {
            let grant = grant.as_document().ok_or(Error::InvalidConfiguration)?;
            let resource = grant
                .get_document("resource")
                .map_err(|_| Error::InvalidConfiguration)?;
            if resource.get_str("collection").ok() != Some(collection) {
                continue;
            }
            if resource.len() != 2 || resource.get_str("db").ok() != Some(db.name()) {
                return Err(Error::InvalidConfiguration);
            }
            for action in grant
                .get_array("actions")
                .map_err(|_| Error::InvalidConfiguration)?
            {
                let action = action.as_str().ok_or(Error::InvalidConfiguration)?;
                if !runtime_actions(collection).contains(&action) {
                    return Err(Error::InvalidConfiguration);
                }
                actual.insert(action);
            }
        }
        if !runtime_actions(collection)
            .iter()
            .all(|a| actual.contains(a))
        {
            return Err(Error::InvalidConfiguration);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_cannot_attest_rewrite_chunks_or_install_schema() {
        let role = role("eg", false);
        for privilege in role.get_array("privileges").unwrap() {
            let p = privilege.as_document().unwrap();
            let c = p
                .get_document("resource")
                .unwrap()
                .get_str("collection")
                .unwrap();
            let actions = p.get_array("actions").unwrap();
            assert!(!actions
                .iter()
                .any(|v| matches!(v.as_str(), Some("createIndex" | "collMod" | "remove"))));
            if [CONTROL, CHUNKS].contains(&c) {
                assert!(!actions.contains(&"update".into()));
            }
        }
    }
    #[test]
    fn ttl_and_paging_definitions_reject_incomplete_index_variants() {
        for required in indexes(EVENTS) {
            assert!(index_matches(&required, &required));
            let mut wrong = required.clone();
            wrong.options.as_mut().unwrap().partial_filter_expression =
                Some(doc! {"scope":"partial"});
            assert!(!index_matches(&required, &wrong));
        }
        assert!(validator(EVENTS)
            .to_string()
            .contains("additionalProperties"));
    }
}
