//! Shared document-schema boundary for versioned Mongo-backed stores.

use std::collections::HashSet;

use futures_util::TryStreamExt;
use layer2_core::StoreError;
use mongodb::bson::{doc, Document};
use mongodb::Database;

pub(super) const DOCUMENT_SCHEMA_VERSION: i64 = 1;
pub(in crate::store) mod index_contract_v1;

/// Create every collection in a fresh database, then reject unsupported rows.
pub(super) async fn initialize_collections(
    database: &Database,
    collections: &[&str],
) -> Result<(), StoreError> {
    ensure_collections(database, collections).await?;
    verify_existing_collections(database, collections).await
}

/// Preserve reader guards without bootstrapping collections for converted stores.
pub(super) async fn verify_existing_collections(
    database: &Database,
    collections: &[&str],
) -> Result<(), StoreError> {
    refuse_unknown_versions(database, collections).await?;
    if collections.contains(&super::work_schema::floor::COLLECTION) {
        super::work_schema::floor::verify(database).await?;
    }
    if collections.contains(&"agents") {
        super::agent_log_segments::schema::verify(database).await?;
        super::turn_timeouts::floor::verify(database).await?;
    }
    Ok(())
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
        let mut unsupported = database
            .collection::<Document>(name)
            .find(unknown_version_filter(name))
            .limit(if session_leases { 0 } else { 1 })
            .projection(if session_leases {
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
            } else {
                return Err(StoreError::Malformed(format!(
                    "collection {name} contains an unsupported document schema version"
                )));
            }
        }
        if *name == super::settings::SETTINGS {
            super::billing_authority::require_legacy_compatible(database).await?;
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

fn unknown_version_filter(collection: &str) -> Document {
    let versions = if collection == "sandboxes" {
        super::work_schema::SANDBOX_VERSIONS.to_vec()
    } else if matches!(collection, "agents" | "agent_reuse_bindings" | "bots") {
        vec![1_i64, 2]
    } else if collection == "sandbox_credentials" {
        vec![
            1_i64,
            super::work_schema::RENEWED_VERSION,
            super::work_schema::PREPARED_VERSION,
            super::turn_timeouts::floor::VERSION,
        ]
    } else if collection == "workspaces" {
        vec![
            1_i64,
            super::work_schema::RENEWED_VERSION,
            super::work_schema::PREPARED_VERSION,
        ]
    } else {
        vec![DOCUMENT_SCHEMA_VERSION]
    };
    let unsupported = doc! {
        "$or": [
            { "document_schema_version": { "$nin": versions } },
            {
                "document_schema_version": {
                    "$not": { "$type": ["int", "long"] }
                }
            },
            { "document_schema_version": { "$type": "array" } },
        ]
    };
    if collection == super::settings::SETTINGS {
        super::billing_authority::exclude_known_envelopes(unsupported)
    } else {
        unsupported
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum SchemaFinding {
    MissingCollection,
    MissingQueryIndex,
    MissingUniqueIndex,
    LegacyBindingIndex,
    TtlAbsent,
    DefinitionMismatch,
    InspectionUnavailable,
}

impl SchemaFinding {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::MissingCollection => "missing_collection",
            Self::MissingQueryIndex => "missing_query_index",
            Self::MissingUniqueIndex => "missing_unique_index",
            Self::LegacyBindingIndex => "legacy_binding_index",
            Self::TtlAbsent => "ttl_absent",
            Self::DefinitionMismatch => "definition_mismatch",
            Self::InspectionUnavailable => "inspection_unavailable",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Contract {
    Binding,
    Activity,
}

impl Contract {
    fn label(self) -> &'static str {
        match self {
            Self::Binding => "binding",
            Self::Activity => "activity",
        }
    }
}

#[derive(Debug)]
pub(super) struct Assessment<T> {
    pub(super) capability: T,
    pub(super) findings: std::collections::BTreeSet<SchemaFinding>,
}

impl<T: Default> Default for Assessment<T> {
    fn default() -> Self {
        Self {
            capability: T::default(),
            findings: Default::default(),
        }
    }
}

/// One accumulated observation budget for both converted contracts in a boot.
pub(super) struct StartupObserver {
    mode: crate::config::StartupSchemaMode,
    remaining: std::time::Duration,
}

impl StartupObserver {
    pub(super) fn new(mode: crate::config::StartupSchemaMode) -> Self {
        Self {
            mode,
            remaining: std::time::Duration::from_secs(5),
        }
    }

    pub(super) async fn assess<T: Default>(
        &mut self,
        contract: Contract,
        inspection: impl std::future::Future<Output = Result<Assessment<T>, SchemaFinding>>,
    ) -> Result<Assessment<T>, StoreError> {
        let started = tokio::time::Instant::now();
        let result = if self.remaining.is_zero() {
            Err(SchemaFinding::InspectionUnavailable)
        } else {
            tokio::time::timeout(self.remaining, inspection)
                .await
                .unwrap_or(Err(SchemaFinding::InspectionUnavailable))
        };
        self.remaining = self.remaining.saturating_sub(started.elapsed());
        let assessment = result.unwrap_or_else(|finding| Assessment {
            capability: T::default(),
            findings: [finding].into(),
        });
        let contract = contract.label();
        let mode = self.mode.label();
        let outcome = if assessment
            .findings
            .contains(&SchemaFinding::InspectionUnavailable)
        {
            "unavailable"
        } else if assessment.findings.is_empty() {
            "compatible"
        } else {
            "drift"
        };
        metrics::counter!("cma_startup_schema_assessments_total", "contract" => contract, "mode" => mode, "outcome" => outcome).increment(1);
        metrics::histogram!("cma_startup_schema_observation_seconds", "contract" => contract, "mode" => mode).record(started.elapsed().as_secs_f64());
        let refuse = self.mode == crate::config::StartupSchemaMode::Enforce
            && !assessment.findings.is_empty();
        for finding in &assessment.findings {
            let reason = finding.label();
            metrics::counter!("cma_startup_schema_findings_total", "contract" => contract, "mode" => mode, "reason" => reason).increment(1);
            tracing::warn!(
                parent: None,
                contract,
                mode,
                reason,
                action = if refuse { "refuse" } else { "continue" },
                "startup schema assessment requires an explicit migration or inspection"
            );
        }
        if refuse {
            return Err(StoreError::Unavailable(format!(
                "{contract} schema {outcome}; explicit migration or inspection required"
            )));
        }
        Ok(assessment)
    }
}

/// Read bounded metadata only; no absent collection or index is created here.
pub(super) async fn inspect_collection(
    database: &Database,
    collection: &str,
) -> Result<Option<Vec<Document>>, SchemaFinding> {
    index_contract_v1::inspect_collection(database, collection)
        .await
        .map_err(|error| match error {
            index_contract_v1::InspectionError::Unavailable => SchemaFinding::InspectionUnavailable,
            index_contract_v1::InspectionError::DefinitionMismatch => {
                SchemaFinding::DefinitionMismatch
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicitly_versioned_aggregates_accept_their_retained_envelopes() {
        let versions = |name| {
            unknown_version_filter(name).get_array("$or").unwrap()[0]["document_schema_version"]
                ["$nin"]
                .clone()
        };
        assert_eq!(versions("agents"), mongodb::bson::bson!([1_i64, 2_i64]));
        assert_eq!(
            versions("agent_reuse_bindings"),
            mongodb::bson::bson!([1_i64, 2_i64])
        );
        assert_eq!(versions("bots"), mongodb::bson::bson!([1_i64, 2_i64]));
        assert_eq!(versions("agent_log"), mongodb::bson::bson!([1_i64]));
        assert_eq!(
            versions("sandboxes"),
            mongodb::bson::bson!([1_i64, 3_i64, 4_i64, 5_i64, 6_i64])
        );
        assert_eq!(
            versions("workspaces"),
            mongodb::bson::bson!([1_i64, 3_i64, 4_i64])
        );
        // Scalar integer and no-array predicates remain present for both paths.
        for guard in [1_usize, 2] {
            assert_eq!(
                unknown_version_filter("agents").get_array("$or").unwrap()[guard],
                unknown_version_filter("other").get_array("$or").unwrap()[guard]
            );
        }
    }

    #[test]
    fn current_version_is_exact_and_the_scan_includes_missing_or_wrong_types() {
        assert!(require_current(1).is_ok());
        assert!(require_current(0).is_err());
        assert!(require_current(2).is_err());
        assert_eq!(
            unknown_version_filter("other")["$or"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }
}
