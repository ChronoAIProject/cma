//! Explicit additive indexes for immutable usage report projections.
use super::Result;
use crate::store::schema::index_contract_v1::{inspect_collection, missing};
use mongodb::{
    bson::{doc, Document},
    Database,
};
use sha2::{Digest, Sha256};

pub(in crate::store) const COLLECTION: &str = "sandbox_usage_report_projections_v1";
pub(super) const ID: &str = "am-usage-report-projections-v1";
pub(super) const INDEX_COUNT: i64 = 4;

pub(super) fn checksum() -> String {
    let mut hash = Sha256::new();
    hash.update(ID);
    hash.update(include_bytes!("usage_projections.rs"));
    hash.update(include_bytes!("../schema/index_contract_v1.rs"));
    format!("{:x}", hash.finalize())
}

fn indexes() -> [Document; INDEX_COUNT as usize] {
    [
        doc! {"name":"usage_projection_key_v1","unique":true,
        "key":{"report_id":1,"generation":1,"kind":1,"metric":1,"series":1,"chunk":1}},
        doc! {"name":"usage_projection_erasure_v1","key":{"report_id":1,"_id":1}},
        doc! {"name":"usage_projection_contributor_v1","key":{"contributor":1,"_id":1}},
        doc! {"name":"usage_projection_expiry_v1","key":{"expires_at":1},"expireAfterSeconds":0},
    ]
}

pub(in crate::store) async fn inspect(
    database: &Database,
) -> Result<Vec<(&'static str, Document)>> {
    let existing = inspect_collection(database, COLLECTION)
        .await
        .map_err(<&str>::from)?
        .unwrap_or_default();
    let desired = indexes();
    if existing.iter().any(|actual| {
        actual.get_str("name").ok() != Some("_id_")
            && !desired
                .iter()
                .any(|wanted| wanted.get("name") == actual.get("name"))
    }) {
        return Err("usage projection index drift");
    }
    let mut absent = Vec::new();
    for index in &desired {
        absent.extend(
            missing(&existing, index)?
                .into_iter()
                .map(|index| (COLLECTION, index)),
        );
    }
    Ok(absent)
}

pub(super) async fn create(database: &Database, requested: Document) -> Result<bool> {
    if !indexes().contains(&requested) {
        return Err("usage projection index mismatch");
    }
    if !inspect(database)
        .await?
        .iter()
        .any(|(_, index)| index == &requested)
    {
        return Ok(false);
    }
    let response = database
        .run_command(doc! {"createIndexes":COLLECTION,"indexes":[requested],
        "maxTimeMS":10_000,"writeConcern":{"w":"majority","wtimeout":5000}})
        .await
        .map_err(|_| "usage projection schema outcome uncertain")?;
    if response.contains_key("writeConcernError") {
        return Err("usage projection schema acknowledgement uncertain");
    }
    Ok(true)
}
