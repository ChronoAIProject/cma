//! Operation-only authority. Runtime startup never acquires these grants.
use super::super::{ledger, Result};
use cma_service_auth::mongodb_authority::{verify_shared_schema, MongoAuthorityMode};
use mongodb::{
    bson::{doc, Document},
    Database,
};

pub(super) async fn verify(
    database: &Database,
    mode: MongoAuthorityMode,
    collections: &[&str],
    build: bool,
) -> Result<()> {
    let status = database
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true,"maxTimeMS":5000})
        .await
        .map_err(|_| "index operation authority unavailable")?;
    crate::store::isolation::verify_index_operation_authority(
        &status,
        database.name(),
        mode,
        collections,
        build,
    )
    .map_err(|_| "index operation authority is insufficient")
}

pub(super) fn abort_role(database: &str, collection: &str) -> Document {
    doc! {"privileges":[
        {"resource":{"db":database,"collection":collection},"actions":["listIndexes","dropIndex"]},
        {"resource":{"db":database,"collection":ledger::COLLECTION},"actions":["find","insert","update","createCollection"]}
    ],"roles":[]}
}

pub(super) fn verify_abort(status: &Document, database: &str, collection: &str) -> Result<()> {
    let permitted = abort_role(database, collection);
    verify_shared_schema(status, database, &permitted)
        .map_err(|_| "abort authority is insufficient")?;
    // Abort uses a dedicated exact role even in a shared schema. It cannot grant
    // database-wide drops or reuse an ordinary runtime's unrelated write access.
    for grant in status
        .get_document("authInfo")
        .map_err(|_| "abort authority malformed")?
        .get_array("authenticatedUserPrivileges")
        .map_err(|_| "abort grants malformed")?
    {
        let grant = grant.as_document().ok_or("abort grant malformed")?;
        let resource = grant
            .get_document("resource")
            .map_err(|_| "abort resource malformed")?;
        let allowed = permitted
            .get_array("privileges")
            .unwrap()
            .iter()
            .filter_map(|p| p.as_document())
            .find(|p| p.get_document("resource").ok() == Some(resource))
            .ok_or("abort requires the exact operator role")?;
        if grant
            .get_array("actions")
            .map_err(|_| "abort actions malformed")?
            .iter()
            .any(|action| !allowed.get_array("actions").unwrap().contains(action))
        {
            return Err("abort credential has unreviewed authority");
        }
    }
    Ok(())
}
