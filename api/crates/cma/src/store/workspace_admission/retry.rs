use super::SandboxStoreError;
use std::time::Duration;

const RETRY_MAX_DELAY_MILLIS: u64 = 32;

pub(in crate::store) async fn retry_delay(attempt: usize) {
    if attempt == 0 {
        return;
    }
    let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(u32::MAX);
    let millis = 1_u64
        .checked_shl(exponent)
        .unwrap_or(u64::MAX)
        .min(RETRY_MAX_DELAY_MILLIS);
    tokio::time::sleep(Duration::from_millis(millis)).await;
}

pub(in crate::store) fn exhausted(operation: &str, last: Option<String>) -> SandboxStoreError {
    SandboxStoreError::Unavailable(format!(
        "{operation} exhausted its retry budget: {}",
        last.unwrap_or_else(|| "no driver result".into())
    ))
}

pub(in crate::store) enum ApplyError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
    Retry(mongodb::error::Error),
}

impl From<super::super::sandbox_events::TransactionEventError> for ApplyError {
    fn from(error: super::super::sandbox_events::TransactionEventError) -> Self {
        use super::super::sandbox_events::TransactionEventError;
        match error {
            TransactionEventError::Domain(error) => Self::Domain(error),
            TransactionEventError::Driver(error) => Self::Driver(error),
        }
    }
}

impl From<super::super::agent_conversation_store::ApplyError> for ApplyError {
    fn from(error: super::super::agent_conversation_store::ApplyError) -> Self {
        use super::super::agent_conversation_store::ApplyError as Budget;
        match error {
            Budget::Domain(error) => Self::Domain(error),
            Budget::Driver(error) => Self::Driver(error),
        }
    }
}
