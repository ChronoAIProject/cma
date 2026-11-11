//! Shared document-schema boundary for versioned Mongo-backed stores.

use std::collections::HashSet;

use futures_util::TryStreamExt;
use layer2_core::StoreError;
use mongodb::bson::{doc, Document};
use mongodb::Database;

pub(super) const DOCUMENT_SCHEMA_VERSION: i64 = 1;

/// Create every collection in a fresh database, then reject unsupported rows.
pub(super) async fn initialize_collections(
    database: &Database,
    collections: &[&str],
) -> Result<(), StoreError> {
    ensure_collections(database, collections).await?;
    refuse_unknown_versions(database, collections).await
}

/// Collection creation shared by stores with explicitly versioned readers.
pub(super) async fn ensure_collections(
    database: &Database,
    collections: &[&str],
) -> Result<(), StoreError> {
    let existing: HashSet<_> = database
        .list_collection_names()
        .await
        .map_err(|error| StoreError::Unavailable(format!("list schema-v1 collections: {error}")))?
        .into_iter()
        .collect();
    for name in collections.iter().filter(|name| !existing.contains(**name)) {
        if let Err(error) = database.create_collection(*name).await {
            let appeared = database
                .list_collection_names()
                .await
                .map_err(|list_error| {
                    StoreError::Unavailable(format!(
                        "recheck schema-v1 collection after create failure: {list_error}"
                    ))
                })?
                .iter()
                .any(|candidate| candidate == name);
            if !appeared {
                return Err(StoreError::Unavailable(format!(
                    "create schema-v1 collection {name}: {error}"
                )));
            }
        }
    }
    Ok(())
}

/// Refuse a database containing any document this binary cannot interpret.
pub(super) async fn refuse_unknown_versions(
    database: &Database,
    collections: &[&str],
) -> Result<(), StoreError> {
    for name in collections {
        let session_leases = *name == super::sandbox_mongo::SANDBOX_SESSIONS;
        let ledger_pre_exclusion = *name == "sandboxes";
        let mut unsupported = database
            .collection::<Document>(name)
            .find(unknown_version_filter())
            .limit(if session_leases || ledger_pre_exclusion {
                0
            } else {
                1
            })
            .projection(if session_leases || ledger_pre_exclusion {
                doc! {} // The complete shape must match the known legacy writer.
            } else {
                doc! { "_id": 1, "document_schema_version": 1 }
            })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("inspect {name} schema version: {error}"))
            })?;
        let mut legacy_count = 0_u64;
        while let Some(document) = unsupported.try_next().await.map_err(|error| {
            StoreError::Unavailable(format!("inspect {name} schema version: {error}"))
        })? {
            if session_leases && super::sandbox_sessions::is_legacy_v1(&document) {
                legacy_count += 1;
            } else if ledger_pre_exclusion
                && super::ledger_maintenance_pre_exclusion_records::read(&document)
                    .is_ok_and(|state| state.is_some())
                && super::sandboxes::read_sandbox(&document).is_ok()
            {
                // Explicit held format; global schema-v1 and other collections stay unchanged.
            } else {
                return Err(StoreError::Malformed(format!(
                    "collection {name} contains an unsupported document schema version"
                )));
            }
        }
        if legacy_count > 0 {
            tracing::warn!(
                parent: None,
                count = legacy_count,
                "accepted legacy v1 sandbox session documents without schema version"
            );
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
