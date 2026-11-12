//! Shared BSON envelope and integer boundaries for model records.
use super::DOCUMENT_SCHEMA_VERSION;
use mongodb::bson::{Bson, Document};
use serde::{de::DeserializeOwned, Serialize};

pub(in crate::store) fn record_bson<T: Serialize>(record: &T) -> Result<Bson, String> {
    mongodb::bson::serialize_to_bson(record).map_err(|error| error.to_string())
}

pub(in crate::store) fn read_record<T: DeserializeOwned>(document: &Document) -> Result<T, String> {
    require_version(document)?;
    let record = document
        .get("record")
        .cloned()
        .ok_or_else(|| "stored document has no aggregate record".to_string())?;
    mongodb::bson::deserialize_from_bson(record).map_err(|error| error.to_string())
}

pub(in crate::store) fn require_version(document: &Document) -> Result<(), String> {
    match document.get("document_schema_version") {
        Some(Bson::Int32(version)) if i64::from(*version) == DOCUMENT_SCHEMA_VERSION => Ok(()),
        Some(Bson::Int64(version)) if *version == DOCUMENT_SCHEMA_VERSION => Ok(()),
        Some(version) => Err(format!("unsupported document schema version {version}")),
        None => Err("stored document has no schema version".into()),
    }
}

pub(in crate::store) fn signed(value: u64, field: &str) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| format!("{field} exceeds the BSON integer range"))
}

pub(in crate::store) fn unsigned(value: i64, field: &str) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| format!("{field} must not be negative"))
}

pub(in crate::store) fn nullable(value: Option<&String>) -> Bson {
    value.cloned().map(Bson::String).unwrap_or(Bson::Null)
}

pub(in crate::store) fn string_field_matches(
    document: &Document,
    field: &str,
    expected: &str,
) -> bool {
    matches!(document.get_str(field), Ok(actual) if actual == expected)
}

pub(in crate::store) fn i64_field_matches(document: &Document, field: &str, expected: i64) -> bool {
    matches!(document.get_i64(field), Ok(actual) if actual == expected)
}
