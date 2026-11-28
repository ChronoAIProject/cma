//! MongoDB storage for platform spend and bounded accounting counters.

use layer2_core::budget::BudgetError;
use mongodb::bson::Document;
use serde::de::DeserializeOwned;

pub mod counters;
pub mod period;

const DOCUMENT_SCHEMA_VERSION: i64 = 1;

pub fn has_turn_certification(record: &Document) -> bool {
    record.contains_key("certification")
}

pub fn decode<T: DeserializeOwned>(document: &Document) -> Result<T, BudgetError> {
    if document
        .get_document("record")
        .is_ok_and(has_turn_certification)
    {
        return Err(BudgetError::Unavailable);
    }
    cma_am_mongo_primitives::read_record(document, DOCUMENT_SCHEMA_VERSION)
        .map_err(|_| BudgetError::Unavailable)
}

pub fn period_key(period_start: u64) -> String {
    let start = std::time::UNIX_EPOCH + std::time::Duration::from_secs(period_start);
    format!("platform:{}", layer2_core::rfc3339_at(start))
}
