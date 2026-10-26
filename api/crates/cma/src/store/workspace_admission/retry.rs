use super::SandboxStoreError;
use std::time::Duration;

const RETRY_MAX_DELAY_MILLIS: u64 = 32;

pub(super) async fn retry_delay(attempt: usize) {
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

pub(super) fn exhausted(operation: &str, last: Option<String>) -> SandboxStoreError {
    SandboxStoreError::Unavailable(format!(
        "{operation} exhausted its retry budget: {}",
        last.unwrap_or_else(|| "no driver result".into())
    ))
}
