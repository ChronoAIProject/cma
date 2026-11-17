//! Explicit additive owner/keyset access path; never installed by serving code.
use super::{index_inventory, Result};
use mongodb::{
    bson::{doc, Document},
    Database,
};
use sha2::{Digest, Sha256};

pub(super) const ID: &str = "am-agent-owner-page-index-v1";
pub(super) fn checksum() -> String {
    format!(
        "{:x}",
        Sha256::digest(include_bytes!("agent_owner_index.rs"))
    )
}
pub(super) fn index() -> Document {
    doc! {"name":"agents_owner_page","key":{"owner":1,"_id":1}}
}
pub(super) async fn inspect(database: &Database) -> Result<Vec<Document>> {
    index_inventory::strict_pending(database, "agents", &index()).await
}
pub(super) async fn create(database: &Database, requested: Document) -> Result<bool> {
    if requested.len() != 2
        || !index_inventory::strict_missing(std::slice::from_ref(&requested), &index())?.is_empty()
    {
        return Err("unreviewed Agent owner index request");
    }
    let response = database
        .run_command(
            doc! {"createIndexes":"agents","indexes":[requested],"maxTimeMS":10000,
            "writeConcern":{"w":"majority","wtimeout":5000}},
        )
        .await
        .map_err(|_| "Agent owner index result uncertain; reconcile before retry")?;
    if response.contains_key("writeConcernError") {
        return Err("Agent owner index acknowledgement uncertain");
    }
    let count = |key| {
        response
            .get_i64(key)
            .or_else(|_| response.get_i32(key).map(i64::from))
            .map_err(|_| "Agent owner index count malformed")
    };
    let before = count("numIndexesBefore")?;
    let after = count("numIndexesAfter")?;
    if before < 0 || after < before || before.checked_add(1).is_none_or(|max| after > max) {
        return Err("Agent owner index count inconsistent");
    }
    Ok(after > before)
}
