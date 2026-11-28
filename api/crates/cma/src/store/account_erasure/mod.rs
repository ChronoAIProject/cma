//! Bounded persistence effects for the account record erasure step.
#![cfg_attr(
    not(test),
    allow(
        dead_code,
        unused_imports,
        reason = "record effects are consumed by the separately delivered account erasure worker"
    )
)]
mod barrier;
mod batches;
pub(crate) mod catalog;
mod context;
mod drain_qualification;
pub(in crate::store) mod erase_source_log;
mod guards;
mod identity_activity;
mod identity_refs;
mod indexed;
mod indexes;
mod optional;
mod profile_refs;
mod progress;
mod records;
mod references;
mod resource_execution;
mod scan;
mod selectors;
pub(crate) use drain_qualification::TransactionDrainQualification;
use layer2_core::{
    account_deletion::{ErasureBlock, ErasureInventory},
    SubjectId,
};
use mongodb::Database;
pub(crate) use records::erase_records as erase_records_with_options;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Options {
    /// Delivery to NyxID's existing provider, independent of prepaid funding.
    pub billing_enabled: bool,
    /// Only an explicitly disabled deployment can omit an absent projection.
    pub allocation_rollups_enabled: Option<bool>,
    pub agent_shared_observation: Option<bool>,
    pub retain_legal_consents: bool,
    pub transaction_drain_minimum_secs: u64,
    pub drain_qualification: Option<TransactionDrainQualification>,
}
impl Options {
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            billing_enabled: config.usage_billing.is_some(),
            allocation_rollups_enabled: Some(config.usage_metrics.views.allocation_rollups_enabled),
            agent_shared_observation: Some(config.features.agent_shared_observation),
            retain_legal_consents: config.retain_legal_consents,
            transaction_drain_minimum_secs: config.account_erasure_transaction_drain_seconds,
            drain_qualification: None,
        }
    }
}
// Test fixtures choose the disabled billing pilot explicitly. Production callers
// must supply the actual deployment mode through erase_records_with_options.
#[cfg(test)]
pub(crate) async fn erase_records(
    db: &Database,
    targets: &[Target<'_>],
    cursor: &mut Cursor,
    now: &str,
) -> Result<Batch, Error> {
    cursor.test_drain_seconds.get_or_insert(0);
    erase_records_with_options(
        db,
        targets,
        cursor,
        now,
        Options {
            drain_qualification: Some(TransactionDrainQualification::fixture()),
            billing_enabled: false,
            allocation_rollups_enabled: Some(false),
            agent_shared_observation: Some(false),
            retain_legal_consents: false,
            transaction_drain_minimum_secs: 120,
        },
    )
    .await
}
#[derive(Debug, Clone)]
pub(crate) struct AttributedBlock {
    pub subject_hash: String,
    pub reason: ErasureBlock,
}
/// Permanent failures without evidence assigning them to a personal account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CohortBlock {
    CorruptRecord {
        collection: &'static str,
    },
    CollectionAuthority {
        collection: &'static str,
        operation: &'static str,
    },
    TransactionDrainQualificationRequired,
    InvalidTransactionDrain {
        parameter: &'static str,
        maximum_seconds: u64,
    },
}
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("record erasure requires cohort intervention: {0:?}")]
    CohortBlocked(CohortBlock),
    #[error("record erasure blocked: {0:?}")]
    Blocked(AttributedBlock),
    #[error("record erasure interrupted after partial progress: {error}")]
    Interrupted {
        error: Box<Error>,
        batch: Box<Batch>,
    },
    #[error("record erasure store unavailable: {0}")]
    Store(String),
    #[error("invalid record in {0}")]
    InvalidRecord(&'static str),
}
impl Error {
    fn cohort(reason: CohortBlock) -> Self {
        tracing::error!(reason = ?reason, "Account erasure requires cohort intervention");
        Self::CohortBlocked(reason)
    }
    fn blocked(subject: &str, reason: ErasureBlock) -> Self {
        Self::Blocked(AttributedBlock {
            subject_hash: layer2_core::document_artifacts::digest(subject.as_bytes()),
            reason,
        })
    }
    fn capacity(subject: &str, class: &'static str, limit: usize) -> Self {
        Self::blocked(
            subject,
            ErasureBlock::CapacityExceeded {
                class: class.into(),
                limit: limit as u64,
            },
        )
    }
    fn attribute(self, subject: &str) -> Self {
        match self {
            Self::InvalidRecord(collection) => Self::blocked(
                subject,
                ErasureBlock::CorruptRecord {
                    collection: collection.into(),
                },
            ),
            other => other,
        }
    }
}
impl From<mongodb::error::Error> for Error {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Store(error.to_string())
    }
}
#[derive(Debug, Default)]
pub(crate) struct Batch {
    pub deleted: BTreeMap<String, u64>,
    pub redacted: BTreeMap<String, u64>,
    pub pending_key_ids: BTreeMap<String, Vec<String>>,
    pub discovered: BTreeMap<String, ErasureInventory>,
    pub has_more: bool,
    pub waiting: bool,
}
pub(crate) struct Target<'a> {
    pub subject: &'a SubjectId,
    pub inventory: &'a ErasureInventory,
}
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct Cursor {
    subjects: BTreeSet<String>,
    catalog_digest: String,
    collection: Option<String>,
    indexed: Option<bool>,
    billing_enabled: Option<bool>,
    operation: usize,
    after: Option<mongodb::bson::Bson>,
    selector: Option<indexed::Key>,
    billing_after: Option<mongodb::bson::Bson>,
    billing_checked: bool,
    contexts: Vec<context::Context>,
    retained_profiles: Option<BTreeSet<String>>,
    retained_profile_owners: BTreeMap<String, String>,
    profile_discovery: Option<profile_refs::Discovery>,
    #[serde(skip)]
    capacity_subject: Option<String>,
    indexes_ready: bool,
    // A rescan marker only: authorization always requires a fresh receipt.
    allocation_absent: Option<bool>,
    observer_absent: Option<bool>,
    usage_projections_absent: Option<bool>,
    execution_namespaces: BTreeSet<String>,
    drain_until: Option<mongodb::bson::DateTime>,
    drain_complete: bool,
    #[cfg(test)]
    #[serde(skip)]
    test_drain_seconds: Option<u64>,
}
impl Cursor {
    fn advance(&mut self) {
        self.operation += 1;
        self.after = None;
        self.selector = None;
    }
}
#[cfg(test)]
mod admission_tests;
#[cfg(test)]
mod batch_tests;
#[cfg(test)]
mod catalog_fixture_tests;
#[cfg(test)]
mod draft_admission_tests;
#[cfg(test)]
mod drain_authority_tests;
#[cfg(test)]
mod history_tests;
#[cfg(test)]
mod identity_pending_tests;
#[cfg(test)]
mod legal_consent_tests;
#[cfg(test)]
mod observer_tests;
#[cfg(test)]
pub(crate) mod optional_authority_tests;
#[cfg(test)]
mod organization_tests;
#[cfg(test)]
mod profile_tests;
#[cfg(test)]
mod race_tests;
#[cfg(test)]
mod reference_tests;
#[cfg(test)]
mod round2_activity_tests;
#[cfg(test)]
mod round2_barrier_tests;
#[cfg(test)]
mod round2_capacity_tests;
#[cfg(test)]
mod round2_hotpath_tests;
#[cfg(test)]
mod round2_profile_integrity_tests;
#[cfg(test)]
mod round2_query_tests;
#[cfg(test)]
mod round2_tests;
#[cfg(test)]
mod round3_barrier_tests;
#[cfg(test)]
mod round3_capacity_tests;
#[cfg(test)]
mod round3_continuation_tests;
#[cfg(test)]
mod round3_discovery_tests;
#[cfg(test)]
mod round4_continuation_tests;
#[cfg(test)]
mod round4_executor_tests;
#[cfg(test)]
mod round4_query_tests;
#[cfg(test)]
mod scan_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(in crate::store) use tests::{drain as test_drain, try_drain as test_try_drain};
