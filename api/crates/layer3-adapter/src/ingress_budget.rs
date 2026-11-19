//! Shared admission for accounted ingress owners; this is not an RSS bound.

use std::sync::{Arc, OnceLock};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const PROCESS_BYTES: usize = 128 * 1024 * 1024;

pub mod failure;
pub mod ownership;
pub mod scope;
pub mod sizing;

pub use failure::{AdmissionRefusal, AdmittedError};
pub use ownership::{AdmittedFrame, AdmittedSocket, ReservationBundle, SharedOwned};
pub use scope::{IngressWorkScope, OwnedWork};
pub use sizing::{AllocationPlan, ClassBounds, IngressClass};

/// One allocation authority shared by adapter and durable-consumer components.
/// Owners still enforce their independent component limits.
#[derive(Clone)]
pub struct IngressBudget(Arc<Semaphore>, Arc<BudgetPolicy>, Option<Arc<Semaphore>>);

struct BudgetPolicy {
    capacity: usize,
    classes: OnceLock<ClassPools>,
}

struct ClassPools {
    bounds: ClassBounds,
    data: Arc<Semaphore>,
    transformation: Arc<Semaphore>,
    control: Arc<Semaphore>,
}

/// Explicit runtime-only policy. Constructing this does not enable any caller.
#[derive(Clone)]
pub struct IngressOwnershipPolicy {
    root: IngressBudget,
    bounds: ClassBounds,
}

impl IngressOwnershipPolicy {
    pub fn new(root: IngressBudget, bounds: ClassBounds) -> crate::Result<Self> {
        if root.2.is_some() || bounds.total_bytes() != root.1.capacity {
            return Err(sizing::invalid());
        }
        // Repeated attachments must share class semaphores, not just the root.
        // A second policy cannot recreate the data ceiling on the same root.
        let classes = root.1.classes.get_or_init(|| ClassPools {
            bounds,
            data: Arc::new(Semaphore::new(bounds.data_bytes())),
            transformation: Arc::new(Semaphore::new(bounds.transformation_bytes())),
            control: Arc::new(Semaphore::new(bounds.control_bytes())),
        });
        if classes.bounds != bounds {
            return Err(sizing::invalid());
        }
        Ok(Self { root, bounds })
    }

    pub fn bounds(&self) -> ClassBounds {
        self.bounds
    }

    /// Existing component pools receive this handle without changing their caps.
    pub fn budget(&self, class: IngressClass) -> IngressBudget {
        let classes = self.root.1.classes.get().expect("policy installs classes");
        let ceiling = match class {
            IngressClass::Data => &classes.data,
            IngressClass::Transformation => &classes.transformation,
            IngressClass::Control => &classes.control,
        };
        IngressBudget(
            self.root.0.clone(),
            self.root.1.clone(),
            Some(ceiling.clone()),
        )
    }

    pub fn reserve(
        &self,
        class: IngressClass,
        plan: AllocationPlan,
    ) -> crate::Result<ReservationBundle> {
        ReservationBundle::try_acquire(&self.budget(class), plan)
    }

    pub fn admit(
        &self,
        class: IngressClass,
        plan: AllocationPlan,
    ) -> Result<ReservationBundle, AdmissionRefusal> {
        ReservationBundle::try_admit(&self.budget(class), plan)
    }
}

impl IngressBudget {
    pub fn process() -> Self {
        static POOL: OnceLock<IngressBudget> = OnceLock::new();
        POOL.get_or_init(|| Self::new(PROCESS_BYTES)).clone()
    }

    pub fn new(bytes: usize) -> Self {
        Self(
            Arc::new(Semaphore::new(bytes)),
            Arc::new(BudgetPolicy {
                capacity: bytes,
                classes: OnceLock::new(),
            }),
            None,
        )
    }

    pub fn available_bytes(&self) -> usize {
        self.0.available_permits().min(
            self.2
                .as_ref()
                .map_or(usize::MAX, |pool| pool.available_permits()),
        )
    }

    pub fn capacity_bytes(&self) -> usize {
        self.1.capacity
    }

    fn try_reserve(&self, bytes: usize) -> Option<AggregateReservation> {
        let bytes = u32::try_from(bytes).ok()?;
        let class = match &self.2 {
            Some(pool) => Some(pool.clone().try_acquire_many_owned(bytes).ok()?),
            None => None,
        };
        let root = self.0.clone().try_acquire_many_owned(bytes).ok()?;
        Some(AggregateReservation { root, class })
    }
}

struct AggregateReservation {
    root: OwnedSemaphorePermit,
    class: Option<OwnedSemaphorePermit>,
}

impl AggregateReservation {
    fn split(&mut self, bytes: usize) -> Option<Self> {
        if bytes > self.root.num_permits() {
            return None;
        }
        Some(Self {
            root: self.root.split(bytes)?,
            class: self.class.as_mut().map(|class| {
                class
                    .split(bytes)
                    .expect("class and root reservations match")
            }),
        })
    }
}

/// Inseparable component and aggregate reservations. Place this after the owned
/// allocation so that storage is freed before either allowance is returned.
pub struct IngressReservation {
    _component: OwnedSemaphorePermit,
    _aggregate: Option<AggregateReservation>,
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
            Some(pool) => Some(pool.try_reserve(bytes as usize)?),
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
