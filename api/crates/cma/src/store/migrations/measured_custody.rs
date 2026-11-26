//! Explicit additive collection and index for measured billing custody.
use mongodb::{
    bson::{doc, Document},
    Database,
};
use sha2::{Digest, Sha256};

use super::Result;
use crate::store::schema::index_contract_v1::{inspect_collection, missing};

pub(in crate::store) const COLLECTION: &str = "sandbox_billing_measured_observations_v1";
pub(super) const ID: &str = "am-measured-billing-custody-v1";

pub(super) fn checksum() -> String {
    let mut hash = Sha256::new();
    hash.update(ID);
    hash.update(include_bytes!("measured_custody.rs"));
    hash.update(include_bytes!("../schema/index_contract_v1.rs"));
    format!("{:x}", hash.finalize())
}

fn index() -> Document {
    doc! {
        "name": "measured_physical_sample_v1",
        "key": {"physical_sample_id": 1, "kind": 1, "_id": 1},
    }
}

pub(super) async fn inspect(database: &Database) -> Result<Vec<(&'static str, Document)>> {
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
        return Err("measured custody index drift");
    }
    Ok(missing(&indexes, &expected)?
        .into_iter()
        .map(|entry| (COLLECTION, entry))
        .collect())
}

pub(super) async fn create(database: &Database, requested: Document) -> Result<bool> {
    if requested != index() {
        return Err("measured custody index mismatch");
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
        .map_err(|_| "measured custody schema outcome uncertain")?;
    if response.contains_key("writeConcernError") {
        return Err("measured custody schema acknowledgement uncertain");
    }
    Ok(true)
}
