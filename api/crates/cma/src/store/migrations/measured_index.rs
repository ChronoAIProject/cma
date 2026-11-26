//! Add the projection and authenticated directory collections independently of v1 custody.
use super::Result;
use crate::store::schema::index_contract_v1::{inspect_collection, missing};
use mongodb::{
    bson::{doc, Document},
    Database,
};
use sha2::{Digest, Sha256};

pub(in crate::store) const PROJECTIONS: &str = "sandbox_billing_measured_projection_v1";
pub(in crate::store) const MANIFESTS: &str = "sandbox_billing_measured_manifest_v1";
pub(in crate::store) const BATCHES: &str = "sandbox_billing_measured_batches_v2";
pub(super) const ID: &str = "am-measured-billing-index-v1";

pub(super) fn checksum() -> String {
    let mut hash = Sha256::new();
    hash.update(ID);
    hash.update(include_bytes!("measured_index.rs"));
    hash.update(include_bytes!("../schema/index_contract_v1.rs"));
    format!("{:x}", hash.finalize())
}

fn indexes(collection: &str) -> Result<Vec<Document>> {
    match collection {
        PROJECTIONS => Ok(vec![doc! {
            "name":"measured_series_hour_v1",
            "key":{"generation":1,"hour":1,"series":1,"timestamp_ms":1,"source_row_id":1},
        }]),
        MANIFESTS => Ok(vec![doc! {
            "name":"measured_manifest_generation_v1",
            "key":{"generation":1},
        }]),
        BATCHES => Ok(vec![
            doc! {"name":"measured_stream_revision_v2",
            "key":{"stream_id":1,"revision":1},"unique":true},
            doc! {"name":"measured_window_overlap_v2",
            "key":{"sandbox_id":1,"from_ms":1,"to_ms":1},
            "partialFilterExpression":{"revision":1_i64}},
            doc! {"name":"measured_window_label_v2",
            "key":{"sandbox_id":1,"window_id":1},"unique":true,
            "partialFilterExpression":{"revision":1_i64}},
        ]),
        _ => Err("measured index collection mismatch"),
    }
}

pub(in crate::store) async fn inspect(
    database: &Database,
) -> Result<Vec<(&'static str, Document)>> {
    let mut required = Vec::new();
    for collection in [PROJECTIONS, MANIFESTS, BATCHES] {
        let expected = indexes(collection)?;
        let Some(existing) = inspect_collection(database, collection)
            .await
            .map_err(<&str>::from)?
        else {
            required.extend(expected.into_iter().map(|index| (collection, index)));
            continue;
        };
        if existing.iter().any(|actual| {
            actual.contains_key("expireAfterSeconds")
                || (actual.get_str("name").ok() != Some("_id_")
                    && !expected
                        .iter()
                        .any(|index| actual.get("name") == index.get("name")))
        }) {
            return Err("measured projection index drift");
        }
        for index in &expected {
            required.extend(
                missing(&existing, index)?
                    .into_iter()
                    .map(|entry| (collection, entry)),
            );
        }
    }
    Ok(required)
}

pub(in crate::store) async fn create(
    database: &Database,
    collection: &str,
    requested: Document,
) -> Result<bool> {
    if !indexes(collection)?.contains(&requested) {
        return Err("measured projection index mismatch");
    }
    if !inspect(database)
        .await?
        .iter()
        .any(|(name, entry)| *name == collection && entry == &requested)
    {
        return Ok(false);
    }
    let response = database
        .run_command(doc! {
            "createIndexes": collection, "indexes": [requested], "maxTimeMS": 10_000,
            "writeConcern": {"w":"majority", "wtimeout":5000},
        })
        .await
        .map_err(|_| "measured projection schema outcome uncertain")?;
    if response.contains_key("writeConcernError") {
        return Err("measured projection schema acknowledgement uncertain");
    }
    Ok(true)
}
