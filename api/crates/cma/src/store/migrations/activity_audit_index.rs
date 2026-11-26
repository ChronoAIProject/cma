//! Explicit additive access path for the audit-first Activity listing.

use mongodb::{
    bson::{doc, Document},
    Database,
};
use sha2::{Digest, Sha256};

use super::{activity_indexes, index_inventory, Result};

pub(super) const ID: &str = "am-activity-audit-index-v1";
pub(in crate::store) const INDEX: &str = "activity_audit_recorded_at_id_desc";
const COLLECTION: &str = "activity";

pub(super) fn checksum() -> String {
    format!(
        "{:x}",
        Sha256::digest(include_bytes!("activity_audit_index.rs"))
    )
}

pub(in crate::store) fn filter() -> Document {
    // The retained codec models three read actions and three outcomes.
    // Preserve unsuccessful reads by including both unsuccessful outcomes.
    doc! {"$or":[
        {"record.action":{"$in":[
            "authenticate", "create", "update", "delete", "configure",
            "gateway_record_write", "gateway_record_delete",
        ]}},
        {"record.outcome.status":{"$in":["refused","failed"]}},
    ]}
}

pub(super) fn index() -> Document {
    // The trailing action preserves the sort prefix and distinguishes this
    // partial index from the existing all-Activity index.
    doc! {"name":INDEX,"key":{"recorded_at":-1,"_id":-1,"record.action":1},
    "partialFilterExpression":filter()}
}

pub(in crate::store) fn ready(existing: &[Document]) -> Result<bool> {
    Ok(index_inventory::strict_missing(existing, &index())?.is_empty())
}

pub(super) async fn inspect(database: &Database) -> Result<Vec<Document>> {
    index_inventory::strict_pending(database, COLLECTION, &index()).await
}

pub(super) async fn create(database: &Database, requested: Document) -> Result<bool> {
    if requested != index() {
        return Err("unreviewed Activity audit index request");
    }
    activity_indexes::create(database, requested).await
}

#[cfg(test)]
pub(in crate::store) async fn prepare_for_test(database: &Database) {
    let pending = inspect(database).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(create(database, pending.into_iter().next().unwrap())
        .await
        .unwrap());
    assert!(inspect(database).await.unwrap().is_empty());
}
