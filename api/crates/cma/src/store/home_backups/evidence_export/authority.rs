use super::{Result, COLLECTIONS};
use mongodb::{
    bson::{doc, Document},
    Database,
};

pub(in crate::store::home_backups) fn role(database: &str) -> Document {
    let mut grants: Vec<_> = COLLECTIONS.iter().map(|name| {
        doc! {"resource": {"db": database, "collection": *name}, "actions": ["find", "listIndexes"]}
    }).collect();
    grants.push(
        doc! {"resource": {"db": database, "collection": ""}, "actions": ["listCollections"]},
    );
    doc! {"createRole": "cma_home_backup_catalog_reader", "privileges": grants, "roles": []}
}

pub(in crate::store::home_backups) fn validate(status: &Document, database: &str) -> Result<()> {
    const DENIED: &str = "catalog_export_authority_refused";
    cma_service_auth::mongodb_authority::verify_shared_schema(status, database, &role(database))
        .map_err(|_| DENIED)?;
    for grant in status
        .get_document("authInfo")
        .map_err(|_| DENIED)?
        .get_array("authenticatedUserPrivileges")
        .map_err(|_| DENIED)?
    {
        let grant = grant.as_document().ok_or(DENIED)?;
        let resource = grant.get_document("resource").map_err(|_| DENIED)?;
        let collection = resource.get_str("collection").map_err(|_| DENIED)?;
        for action in grant.get_array("actions").map_err(|_| DENIED)? {
            let action = action.as_str().ok_or(DENIED)?;
            if !(if collection.is_empty() {
                action == "listCollections"
            } else {
                COLLECTIONS.contains(&collection) && ["find", "listIndexes"].contains(&action)
            }) {
                return Err(DENIED);
            }
        }
    }
    Ok(())
}

pub(super) async fn verify(database: &Database) -> Result<()> {
    let status = database
        .run_command(doc! {"connectionStatus": 1, "showPrivileges": true})
        .await
        .map_err(|_| "catalog_export_authority_refused")?;
    validate(&status, database.name())
}
