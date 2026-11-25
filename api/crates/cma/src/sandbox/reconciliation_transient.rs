//! Attempt-scoped recognition of runtime failures at startup binding.
//!
//! Runtime and store failures share `SandboxStoreError::Unavailable`. An
//! attempt records the classified adapter failure and uses it only if the
//! attempt returns that mapped store error.
use std::{cell::RefCell, future::Future};

use layer2_core::sandbox::SandboxStoreError;
use layer3_adapter::AdapterError;

tokio::task_local! {
    static FAILURE: RefCell<Option<(String, StartupFailure)>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StartupFailure {
    Unavailable,
    NativeHomeIdentity,
}

pub(super) fn provider_error(error: AdapterError) -> SandboxStoreError {
    let failure =
        matches!(error, AdapterError::Unavailable(_)).then_some(StartupFailure::Unavailable);
    record(error, failure)
}

pub(super) fn native_home_identity(error: AdapterError) -> SandboxStoreError {
    record(error, Some(StartupFailure::NativeHomeIdentity))
}

fn record(error: AdapterError, failure: Option<StartupFailure>) -> SandboxStoreError {
    let mapped = SandboxStoreError::Unavailable(format!(
        "sandbox runtime authority is unavailable: {error}"
    ));
    if let (Some(failure), SandboxStoreError::Unavailable(message)) = (failure, &mapped) {
        let _ = FAILURE.try_with(|slot| *slot.borrow_mut() = Some((message.clone(), failure)));
    }
    mapped
}

pub(super) async fn classify<T>(
    attempt: impl Future<Output = Result<T, SandboxStoreError>>,
) -> (Result<T, SandboxStoreError>, Option<StartupFailure>) {
    FAILURE
        .scope(RefCell::new(None), async {
            let result = attempt.await;
            let failure = match &result {
                Err(SandboxStoreError::Unavailable(message)) => FAILURE.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .filter(|(recorded, _)| recorded == message)
                        .map(|(_, failure)| *failure)
                }),
                _ => None,
            };
            (result, failure)
        })
        .await
}

#[cfg(test)]
#[path = "reconciliation_transient_tests.rs"]
mod tests;
