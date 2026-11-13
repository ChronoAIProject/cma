//! Preserve title transaction and reconciliation error classifications.
use super::*;

pub(super) fn map_domain(error: SandboxStoreError) -> ApplyError {
    ApplyError::Domain(map_store_error(error))
}

pub(super) fn map_transaction_log(error: TransactionLogError) -> ApplyError {
    match error {
        TransactionLogError::Domain(error) => map_domain(error),
        TransactionLogError::Driver(error) => ApplyError::Driver(error),
    }
}

pub(super) fn map_reconciliation_log(error: TransactionLogError) -> AgentTitleStoreError {
    match error {
        TransactionLogError::Domain(error) => map_store_error(error),
        TransactionLogError::Driver(_) => AgentTitleStoreError::Unavailable,
    }
}
