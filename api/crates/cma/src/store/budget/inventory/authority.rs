use super::{digest, Result, COLLECTIONS};
use mongodb::{
    bson::{doc, Bson, Document},
    Database,
};

const AUTHORITY: &str = "turn_inventory_read_only_authority_required";
const SCOPE: &str = "turn_inventory_existing_collections_required";

fn role(database: &str) -> Document {
    let mut grants: Vec<_> = COLLECTIONS
        .iter()
        .map(|name| {
            doc! {"resource": {"db": database, "collection": *name}, "actions": ["find"]}
        })
        .collect();
    grants.push(
        doc! {"resource": {"db": database, "collection": ""}, "actions": ["listCollections"]},
    );
    doc! {"privileges": grants, "roles": []}
}

pub(super) fn validate(status: &Document, database: &str) -> Result<()> {
    cma_service_auth::mongodb_authority::verify_shared_schema(status, database, &role(database))
        .map_err(|_| AUTHORITY)?;
    for grant in status
        .get_document("authInfo")
        .map_err(|_| AUTHORITY)?
        .get_array("authenticatedUserPrivileges")
        .map_err(|_| AUTHORITY)?
    {
        let grant = grant.as_document().ok_or(AUTHORITY)?;
        let collection = grant
            .get_document("resource")
            .map_err(|_| AUTHORITY)?
            .get_str("collection")
            .map_err(|_| AUTHORITY)?;
        for action in grant.get_array("actions").map_err(|_| AUTHORITY)? {
            let action = action.as_str().ok_or(AUTHORITY)?;
            if !(if collection.is_empty() {
                action == "listCollections"
            } else {
                COLLECTIONS.contains(&collection) && action == "find"
            }) {
                return Err(AUTHORITY);
            }
        }
    }
    Ok(())
}

pub(super) async fn verify(database: &Database) -> Result<()> {
    let status = database
        .run_command(doc! {"connectionStatus": 1, "showPrivileges": true})
        .await
        .map_err(|_| AUTHORITY)?;
    validate(&status, database.name())
}

pub(super) async fn scope(database: &Database) -> Result<String> {
    let mut identities = vec![doc! {"database": database.name()}];
    for name in COLLECTIONS {
        let response = database
            .run_command(doc! {"listCollections": 1,
            "filter": {"name": name}, "cursor": {"batchSize": 2}, "maxTimeMS": 5000})
            .await
            .map_err(|_| SCOPE)?;
        let rows =
            crate::store::schema::index_contract_v1::entries(&response).map_err(|_| SCOPE)?;
        if rows.len() != 1
            || rows[0].get_str("name").ok() != Some(name)
            || rows[0].get_str("type").ok() != Some("collection")
        {
            return Err(SCOPE);
        }
        let uuid = rows[0]
            .get_document("info")
            .map_err(|_| SCOPE)?
            .get("uuid")
            .ok_or(SCOPE)?;
        if !matches!(uuid, Bson::Binary(_)) {
            return Err(SCOPE);
        }
        identities.push(doc! {"name": name, "uuid": uuid.clone()});
    }
    Ok(digest(
        &mongodb::bson::serialize_to_vec(&doc! {"scope": identities}).map_err(|_| SCOPE)?,
    ))
}

#[cfg(test)]
pub(super) fn test_role(database: &str) -> Document {
    let mut value = doc! {"createRole": "turn-inventory-reader"};
    value.extend(role(database));
    value
}
