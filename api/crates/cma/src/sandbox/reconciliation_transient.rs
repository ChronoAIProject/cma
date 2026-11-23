//! Attempt-scoped recognition of a transient runtime failure at startup binding.
//!
//! Runtime and store failures share `SandboxStoreError::Unavailable`. Within one
//! startup binding attempt, `provider_error` records the exact error it mapped
//! from a transient `AdapterError::Unavailable`; the attempt is transient only
//! when its returned error is that same value. Anything else, including a store
//! failure, a non-transient runtime refusal, a failure mapped inside a spawned
//! task or any call outside an attempt, stays fatal exactly as before.
use std::{cell::RefCell, future::Future};

use layer2_core::sandbox::SandboxStoreError;
use layer3_adapter::AdapterError;

tokio::task_local! {
    static TRANSIENT: RefCell<Option<String>>;
}

/// Record the mapped error of a transient runtime failure, if inside an attempt.
pub(super) fn note(error: &AdapterError, mapped: &SandboxStoreError) {
    if let (AdapterError::Unavailable(_), SandboxStoreError::Unavailable(message)) = (error, mapped)
    {
        let _ = TRANSIENT.try_with(|slot| *slot.borrow_mut() = Some(message.clone()));
    }
}

/// Run one attempt and report whether its error is exactly a recorded
/// transient runtime failure from this attempt.
pub(super) async fn classify<T>(
    attempt: impl Future<Output = Result<T, SandboxStoreError>>,
) -> (Result<T, SandboxStoreError>, bool) {
    TRANSIENT
        .scope(RefCell::new(None), async {
            let result = attempt.await;
            let transient = match &result {
                Err(SandboxStoreError::Unavailable(message)) => {
                    TRANSIENT.with(|slot| slot.borrow().as_deref() == Some(message.as_str()))
                }
                _ => false,
            };
            (result, transient)
        })
        .await
}

#[cfg(test)]
#[path = "reconciliation_transient_tests.rs"]
mod tests;
