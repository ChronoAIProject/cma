//! Explicit additive collection and index for finalized measured billing evidence.
use mongodb::{
    bson::{doc, Document},
    Database,
};
use sha2::{Digest, Sha256};

use super::Result;
use crate::store::schema::index_contract_v1::{inspect_collection, missing};

pub(in crate::store) const COLLECTION: &str = "sandbox_billing_measured_batches_v1";
pub(super) const ID: &str = "am-measured-billing-batches-v1";

pub(super) fn checksum() -> String {
    let mut hash = Sha256::new();
    hash.update(ID);
    hash.update(include_bytes!("measured_batches.rs"));
    hash.update(include_bytes!("../schema/index_contract_v1.rs"));
    format!("{:x}", hash.finalize())
}

fn indexes() -> [Document; 3] {
    [
        doc! {
            "name": "measured_stream_revision_v1",
            "key": {"stream_id": 1, "revision": 1},
            "unique": true,
        },
        doc! {
            "name": "measured_window_seconds_v1",
            "key": {"sandbox_id": 1, "window_seconds": 1},
            "unique": true,
            "partialFilterExpression": {"revision": 1_i64},
        },
        doc! {
            "name": "measured_window_label_v1",
            "key": {"sandbox_id": 1, "window_id": 1},
            "unique": true,
            "partialFilterExpression": {"revision": 1_i64},
        },
    ]
}

pub(in crate::store) async fn inspect(
    database: &Database,
) -> Result<Vec<(&'static str, Document)>> {
    let Some(existing) = inspect_collection(database, COLLECTION)
        .await
        .map_err(<&str>::from)?
    else {
        return Ok(indexes()
            .into_iter()
            .map(|index| (COLLECTION, index))
            .collect());
    };
    let expected = indexes();
    if existing.iter().any(|actual| {
        actual.contains_key("expireAfterSeconds")
            || (actual.get_str("name").ok() != Some("_id_")
                && !expected
                    .iter()
                    .any(|index| actual.get("name") == index.get("name")))
    }) {
        return Err("measured batch index drift");
    }
    let mut required = Vec::new();
    for index in &expected {
        required.extend(
            missing(&existing, index)?
                .into_iter()
                .map(|entry| (COLLECTION, entry)),
        );
    }
    Ok(required)
}

pub(in crate::store) async fn create(database: &Database, requested: Document) -> Result<bool> {
    if !indexes().contains(&requested) {
        return Err("measured batch index mismatch");
    }
    if !inspect(database)
        .await?
        .iter()
        .any(|(_, index)| index == &requested)
    {
        return Ok(false);
    }
    let response = database
        .run_command(doc! {
            "createIndexes": COLLECTION, "indexes": [requested], "maxTimeMS": 10_000,
            "writeConcern": {"w": "majority", "wtimeout": 5000},
        })
        .await
        .map_err(|_| "measured batch schema outcome uncertain")?;
    if response.contains_key("writeConcernError") {
        return Err("measured batch schema acknowledgement uncertain");
    }
    Ok(true)
}
