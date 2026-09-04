//! Shared document-schema boundary for versioned Mongo-backed stores.

use layer2_core::StoreError;
use mongodb::bson::{doc, Document};
use mongodb::Database;

pub(super) const DOCUMENT_SCHEMA_VERSION: i64 = 1;

/// Refuse a database containing any document this binary cannot interpret.
pub(super) async fn refuse_unknown_versions(
    database: &Database,
    collections: &[&str],
) -> Result<(), StoreError> {
    for name in collections {
        let unsupported = database
            .collection::<Document>(name)
            .find_one(unknown_version_filter())
            .projection(doc! { "_id": 1, "document_schema_version": 1 })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("inspect {name} schema version: {error}"))
            })?;
        if unsupported.is_some() {
            return Err(StoreError::Malformed(format!(
                "collection {name} contains an unsupported document schema version"
            )));
        }
    }
    Ok(())
}

pub(super) fn require_current(version: i64) -> Result<(), StoreError> {
    if version == DOCUMENT_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(StoreError::Malformed(format!(
            "unsupported document schema version {version}"
        )))
    }
}

fn unknown_version_filter() -> Document {
    doc! {
        "$or": [
            { "document_schema_version": { "$ne": DOCUMENT_SCHEMA_VERSION } },
            {
                "document_schema_version": {
                    "$not": { "$type": ["int", "long"] }
                }
            },
            { "document_schema_version": { "$type": "array" } },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_version_is_exact_and_the_scan_includes_missing_or_wrong_types() {
        assert!(require_current(1).is_ok());
        assert!(require_current(0).is_err());
        assert!(require_current(2).is_err());
        assert_eq!(unknown_version_filter()["$or"].as_array().unwrap().len(), 3);
    }
}
