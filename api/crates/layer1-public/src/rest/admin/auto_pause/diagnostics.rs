use layer2_core::sandbox::store::SandboxStoreError;

use super::CollectionProblem;

pub(super) fn record(stage: &'static str, failure: &'static str) {
    // The enclosing request span supplies correlation. Store error messages can
    // contain durable data or driver details, so emit fixed categories only.
    tracing::warn!(
        target: "cma::admin::dependencies",
        operation = "auto_pause_preview",
        stage,
        failure,
        "operator preview dependency failed"
    );
}

pub(super) fn unavailable(stage: &'static str, failure: &'static str) -> CollectionProblem {
    record(stage, failure);
    CollectionProblem::Unavailable
}

pub(super) fn store_failure(stage: &'static str, error: &SandboxStoreError) -> CollectionProblem {
    let failure = match error {
        SandboxStoreError::Conflict => "revision_conflict",
        SandboxStoreError::SequenceConflict => "sequence_conflict",
        SandboxStoreError::NotFound => "not_found",
        SandboxStoreError::InvalidPage => "invalid_page",
        SandboxStoreError::InvalidTransaction(_) => "invalid_transaction",
        SandboxStoreError::Unavailable(_) => "store_unavailable",
        SandboxStoreError::Corrupt(_) => "corrupt_data",
        _ => "domain_refusal",
    };
    unavailable(stage, failure)
}
