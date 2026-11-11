//! Runtime retirement cannot erase segmented source or its ownership lineage.
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Bson},
    ClientSession,
};

use super::{collections::collection, COLLECTIONS, HEADS, PROJECTIONS};
use crate::store::sandbox_mongo::MongoSandboxStore;

pub(in crate::store) async fn required(
    store: &MongoSandboxStore,
    sandbox_id: &str,
    session: &mut ClientSession,
) -> mongodb::error::Result<bool> {
    // Check the original aggregate as well as the query projection. Damaged
    // projection fields must not erase the only retained ownership lineage.
    let mut cursor = store
        .agents
        .find(doc! {"$or":[
            {"sandbox_id":sandbox_id},{"record.sandbox_id":sandbox_id},
        ]})
        .projection(doc! {"_id":1,"document_schema_version":1,
        "agent_log_format":1,"agent_log_incarnation":1,"agent_log_head_revision":1})
        .batch_size(16)
        .session(&mut *session)
        .await?;
    loop {
        let next = cursor.stream(&mut *session).try_next().await?;
        let Some(row) = next else {
            break;
        };
        // Existing legacy purge includes pre-envelope records. Preserve that
        // behavior only in the absence of every segment marker and sidecar.
        if !matches!(
            row.get("document_schema_version"),
            None | Some(Bson::Int32(1) | Bson::Int64(1))
        ) || [
            "agent_log_format",
            "agent_log_incarnation",
            "agent_log_head_revision",
        ]
        .iter()
        .any(|key| row.contains_key(*key))
        {
            return Ok(true);
        }
        let Ok(agent_id) = row.get_str("_id") else {
            return Ok(true);
        };
        // Markers can be damaged independently of retained sidecars. Retain
        // even an apparently legacy Agent when any new-format evidence exists.
        for name in COLLECTIONS {
            let sidecar = collection(store, name);
            let field = if matches!(*name, HEADS | PROJECTIONS) {
                "_id"
            } else {
                "agent_id"
            };
            let filter = doc! {field:agent_id};
            if sidecar
                .find_one(filter)
                .projection(doc! {"_id":1})
                .session(&mut *session)
                .await?
                .is_some()
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
