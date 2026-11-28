//! BSON model records use the Agent Manager's current schema version.
use super::DOCUMENT_SCHEMA_VERSION;
pub(in crate::store) use cma_am_mongo_primitives::{
    i64_field_matches, nullable, record_bson, signed, string_field_matches, unsigned,
};
use mongodb::bson::Document;
use serde::de::DeserializeOwned;

pub(in crate::store) fn read_record<T: DeserializeOwned>(document: &Document) -> Result<T, String> {
    cma_am_mongo_primitives::read_record(document, DOCUMENT_SCHEMA_VERSION)
}

pub(in crate::store) fn require_version(document: &Document) -> Result<(), String> {
    cma_am_mongo_primitives::require_version(document, DOCUMENT_SCHEMA_VERSION)
}
