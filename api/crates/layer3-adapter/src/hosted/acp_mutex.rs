use std::ops::Deref;

use crate::ingress_budget::{
    AdmissionRefusal, AllocationPlan, IngressClass, IngressOwnershipPolicy, ReservationBundle,
};

/// Retain the mutex's backing until the mutex itself is physically destroyed,
/// including when an Arc clone outlives the Bridge. Payload storage is separate.
pub(in crate::hosted) struct BridgeMutex<T> {
    inner: tokio::sync::Mutex<T>,
    _storage: Option<ReservationBundle>,
}

impl<T> BridgeMutex<T> {
    pub(super) fn new(
        value: T,
        policy: Option<&IngressOwnershipPolicy>,
    ) -> Result<Self, AdmissionRefusal> {
        let storage = policy
            .map(|policy| {
                if !cfg!(any(target_os = "linux", target_vendor = "apple")) {
                    return Err(AdmissionRefusal::InvalidPlan {
                        phase: "unsupported bridge mutex layout",
                    });
                }
                // Tokio 1.53.3's semaphore uses a std wait-list mutex. Rust
                // 1.96.1 on Apple lazily boxes pthread_mutex_t on first unlock;
                // Linux's corresponding futex storage is inline.
                #[cfg(target_vendor = "apple")]
                let backing = size_of::<libc::pthread_mutex_t>();
                #[cfg(not(target_vendor = "apple"))]
                let backing = 0;
                policy.admit(
                    IngressClass::Data,
                    AllocationPlan::checked("bridge mutex backing", &[backing])?,
                )
            })
            .transpose()?;
        let inner = tokio::sync::Mutex::new(value);
        if storage.is_some() {
            // Initialize inside its admitted construction phase so first-use
            // accounting cannot mistake persistent backing for query scratch.
            drop(inner.try_lock().expect("new mutex is uncontended"));
        }
        Ok(Self {
            inner,
            _storage: storage,
        })
    }
}

impl<T> Deref for BridgeMutex<T> {
    type Target = tokio::sync::Mutex<T>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[cfg(test)]
#[path = "acp_mutex_tests.rs"]
mod tests;
