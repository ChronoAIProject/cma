//! BSON model records use the Agent Manager's current schema version.
use super::DOCUMENT_SCHEMA_VERSION;
pub(in crate::store) use cma_am_mongo_primitives::{record_bson, signed, unsigned};
use mongodb::bson::{Bson, Document};
use serde::de::DeserializeOwned;

pub(in crate::store) fn read_record<T: DeserializeOwned>(document: &Document) -> Result<T, String> {
    cma_am_mongo_primitives::read_record(document, DOCUMENT_SCHEMA_VERSION)
}

pub(in crate::store) fn require_version(document: &Document) -> Result<(), String> {
    cma_am_mongo_primitives::require_version(document, DOCUMENT_SCHEMA_VERSION)
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
