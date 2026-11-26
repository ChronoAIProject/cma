//! Add the revision-bound active log proof independently of the sealed log format.
use mongodb::{
    bson::{doc, Document},
    Database,
};
use sha2::{Digest, Sha256};

use super::Result;
use crate::store::schema::index_contract_v1::{inspect_collection, missing};

pub(in crate::store) use crate::store::agent_log_segments::active_proof::COLLECTION;
pub(super) const ID: &str = "am-agent-log-active-proofs-v1";

pub(super) fn checksum() -> String {
    let mut hash = Sha256::new();
    hash.update(ID);
    hash.update(include_bytes!("agent_log_active_proofs.rs"));
    hash.update(include_bytes!("../schema/index_contract_v1.rs"));
    format!("{:x}", hash.finalize())
}

fn index() -> Document {
    doc! {
        "name": "agent_log_active_proof_incarnation",
        "key": {"head.incarnation": 1, "head.revision": 1},
    }
}

pub(in crate::store) async fn inspect(
    database: &Database,
) -> Result<Vec<(&'static str, Document)>> {
    let Some(indexes) = inspect_collection(database, COLLECTION)
        .await
        .map_err(<&str>::from)?
    else {
        return Ok(vec![(COLLECTION, index())]);
    };
    let expected = index();
    if indexes.iter().any(|actual| {
        actual.contains_key("expireAfterSeconds")
            || (actual.get_str("name").ok() != Some("_id_")
                && actual.get("name") != expected.get("name"))
    }) {
        return Err("active log proof index drift");
    }
    Ok(missing(&indexes, &expected)?
        .into_iter()
        .map(|entry| (COLLECTION, entry))
        .collect())
}

pub(in crate::store) async fn create(database: &Database, requested: Document) -> Result<bool> {
    if requested != index() {
        return Err("active log proof index mismatch");
    }
    if inspect(database).await?.is_empty() {
        return Ok(false);
    }
    let response = database
        .run_command(doc! {
            "createIndexes": COLLECTION, "indexes": [requested], "maxTimeMS": 10_000,
            "writeConcern": {"w": "majority", "wtimeout": 5000},
        })
        .await
        .map_err(|_| "active log proof schema outcome uncertain")?;
    if response.contains_key("writeConcernError") {
        return Err("active log proof schema acknowledgement uncertain");
    }
    Ok(true)
}
