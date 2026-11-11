use cma_service_auth::mongodb_authority::{verify_shared_schema, MongoAuthorityMode};
use mongodb::bson::{doc, Document};

use super::{ledger::COLLECTION, Result};

pub(super) fn role(database: &str, apply: bool) -> Document {
    doc! {"createRole":"cma_am_migrator","privileges":[
        {"resource":{"db":database,"collection":COLLECTION},
         "actions":if apply {vec!["find","insert","update","createCollection"]} else {vec!["find"]}},
        {"resource":{"db":database,"collection":"activity"},
         "actions":if apply {vec!["listIndexes","createIndex","createCollection"]} else {vec!["listIndexes"]}}
    ],"roles":[]}
}

pub(super) fn verify(
    status: &Document,
    database: &str,
    mode: MongoAuthorityMode,
    apply: bool,
) -> Result<()> {
    verify_shared_schema(status, database, &role(database, apply))
        .map_err(|_| "migration database authority is insufficient")?;
    if mode == MongoAuthorityMode::Isolated {
        let info = status
            .get_document("authInfo")
            .map_err(|_| "migration authority malformed")?;
        let permitted = role(database, true);
        for grant in info
            .get_array("authenticatedUserPrivileges")
            .map_err(|_| "migration grants malformed")?
        {
            let grant = grant.as_document().ok_or("migration grant malformed")?;
            let resource = grant
                .get_document("resource")
                .map_err(|_| "migration resource malformed")?;
            let expected = permitted
                .get_array("privileges")
                .unwrap()
                .iter()
                .filter_map(|value| value.as_document())
                .find(|value| value.get_document("resource").ok() == Some(resource))
                .ok_or("migration credential is not service isolated")?;
            let allowed = expected.get_array("actions").unwrap();
            if grant
                .get_array("actions")
                .map_err(|_| "migration actions malformed")?
                .iter()
                .any(|action| !allowed.contains(action))
            {
                return Err("migration credential has unreviewed authority");
            }
        }
    }
    Ok(())
}
