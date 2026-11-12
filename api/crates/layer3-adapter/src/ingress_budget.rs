//! Shared admission for accounted ingress owners; this is not an RSS bound.

use std::sync::{Arc, OnceLock};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const PROCESS_BYTES: usize = 128 * 1024 * 1024;

/// One allocation authority shared by adapter and durable-consumer components.
/// Owners still enforce their independent component limits.
#[derive(Clone)]
pub struct IngressBudget(Arc<Semaphore>);

impl IngressBudget {
    pub fn process() -> Self {
        static POOL: OnceLock<IngressBudget> = OnceLock::new();
        POOL.get_or_init(|| Self::new(PROCESS_BYTES)).clone()
    }

    pub fn new(bytes: usize) -> Self {
        Self(Arc::new(Semaphore::new(bytes)))
    }

    pub fn available_bytes(&self) -> usize {
        self.0.available_permits()
    }
}

/// Inseparable component and aggregate reservations. Place this after the owned
/// allocation so that storage is freed before either allowance is returned.
pub struct IngressReservation {
    _component: OwnedSemaphorePermit,
    _aggregate: Option<OwnedSemaphorePermit>,
}

impl IngressReservation {
    /// Never waits while holding another reservation. Any failed acquisition
    /// rolls back the partial component reservation before returning refusal.
    pub fn try_acquire(
        component: &Arc<Semaphore>,
        aggregate: Option<&IngressBudget>,
        bytes: usize,
    ) -> Option<Self> {
        let bytes = u32::try_from(bytes).ok()?;
        let component = component.clone().try_acquire_many_owned(bytes).ok()?;
        let aggregate = match aggregate {
            Some(pool) => Some(pool.0.clone().try_acquire_many_owned(bytes).ok()?),
            None => None,
        };
        Some(Self {
            _component: component,
            _aggregate: aggregate,
        })
    }
}

#[cfg(test)]
#[path = "ingress_budget_tests.rs"]
mod tests;
