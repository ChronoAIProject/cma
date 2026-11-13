//! Call-local timeout classification; a past observation never covers a new read.
use crate::{AdapterError, Result};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

/// Records whether a deadline interrupted polling after a validated Pending
/// result, or interrupted an outstanding/erroring dependency observation.
#[derive(Default)]
pub struct NativeReadinessObservation {
    pending: AtomicBool,
}

impl NativeReadinessObservation {
    /// The caller returns `None` only after validating the exact resource and
    /// observing it Pending. Starting another read invalidates that evidence.
    pub async fn observe<T>(
        &self,
        operation: impl Future<Output = Result<Option<T>>>,
    ) -> Result<Option<T>> {
        self.pending.store(false, Ordering::Relaxed);
        let result = operation.await;
        self.pending
            .store(matches!(result, Ok(None)), Ordering::Relaxed);
        result
    }

    pub fn timeout(&self) -> AdapterError {
        AdapterError::NativeClaimNotReady(if self.pending.load(Ordering::Relaxed) {
            super::NativeClaimGuardReason::Pending
        } else {
            super::NativeClaimGuardReason::TimedOut
        })
    }
}
