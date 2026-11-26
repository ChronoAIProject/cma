//! Shared BSON, durability, and transaction primitives for Agent Manager stores.

use bson::{Bson, Document};
use serde::{de::DeserializeOwned, Serialize};

pub mod topology;
pub mod transaction;

mod durability;
pub use durability::majority_collection;

pub fn record_bson<T: Serialize>(record: &T) -> Result<Bson, String> {
    bson::serialize_to_bson(record).map_err(|error| error.to_string())
}

pub fn read_record<T: DeserializeOwned>(
    document: &Document,
    expected_version: i64,
) -> Result<T, String> {
    require_version(document, expected_version)?;
    let record = document
        .get("record")
        .cloned()
        .ok_or_else(|| "stored document has no aggregate record".to_string())?;
    bson::deserialize_from_bson(record).map_err(|error| error.to_string())
}

pub fn require_version(document: &Document, expected_version: i64) -> Result<(), String> {
    match document.get("document_schema_version") {
        Some(Bson::Int32(version)) if i64::from(*version) == expected_version => Ok(()),
        Some(Bson::Int64(version)) if *version == expected_version => Ok(()),
        Some(version) => Err(format!("unsupported document schema version {version}")),
        None => Err("stored document has no schema version".into()),
    }
}

pub fn signed(value: u64, field: &str) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| format!("{field} exceeds the BSON integer range"))
}

pub fn unsigned(value: i64, field: &str) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| format!("{field} must not be negative"))
}
