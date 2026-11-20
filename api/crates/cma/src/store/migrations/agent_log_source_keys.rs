//! Explicit additive access path for codec-invalid sequence keys.
use super::{index_inventory, Result};
use mongodb::{
    bson::{doc, Document},
    Database,
};
use sha2::{Digest, Sha256};

pub(super) const ID: &str = "am-agent-log-source-key-index-v1";
pub(in crate::store) const INDEX: &str = "agent_log_malformed_sequence";

pub(super) fn checksum() -> String {
    format!(
        "{:x}",
        Sha256::digest(include_bytes!("agent_log_source_keys.rs"))
    )
}

pub(super) fn index() -> Document {
    // The codec accepts only BSON Int64 (18). Null equality also indexes an
    // absent field; array type includes empty and all-Int64 arrays.
    doc! {"name":INDEX,"key":{"agent_id":1},"partialFilterExpression":{"$or":[
        {"seq":null},
        {"seq":{"$type":[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,19,-1,127]}},
    ]}}
}

pub(in crate::store) async fn inspect(database: &Database) -> Result<Vec<Document>> {
    index_inventory::strict_pending(database, "agent_log", &index()).await
}

pub(super) async fn create(database: &Database, requested: Document) -> Result<bool> {
    if requested != index() {
        return Err("unreviewed Agent log source-key index request");
    }
    let response = database
        .run_command(
            doc! {"createIndexes":"agent_log","indexes":[requested],"maxTimeMS":10000,
            "writeConcern":{"w":"majority","wtimeout":5000}},
        )
        .await
        .map_err(|_| "Agent log source-key index result uncertain; reconcile before retry")?;
    if response.contains_key("writeConcernError") {
        return Err("Agent log source-key index acknowledgement uncertain");
    }
    let count = |key| {
        response
            .get_i64(key)
            .or_else(|_| response.get_i32(key).map(i64::from))
            .map_err(|_| "Agent log source-key index count malformed")
    };
    let before = count("numIndexesBefore")?;
    let after = count("numIndexesAfter")?;
    if before < 0 || after < before || before.checked_add(1).is_none_or(|max| after > max) {
        return Err("Agent log source-key index count inconsistent");
    }
    Ok(after > before)
}

#[cfg(test)]
pub(in crate::store) async fn prepare_for_test(database: &Database) {
    for index in inspect(database).await.unwrap() {
        create(database, index).await.unwrap();
    }
}
