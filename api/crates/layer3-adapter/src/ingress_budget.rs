//! Shared admission for accounted ingress owners; this is not an RSS bound.

use std::sync::{atomic::AtomicBool, Arc, OnceLock};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const PROCESS_BYTES: usize = 128 * 1024 * 1024;

mod attachment;
mod classes;
mod component;
pub(crate) mod custody;
pub(crate) mod diagnostic;
pub mod failure;
pub mod observation;
pub mod operation;
pub mod ownership;
mod root_storage;
pub mod scope;
pub mod sizing;
mod source_erasure;
pub mod watch;

pub use crate::conversation_channel::admitted::{
    AdmittedConversationChannel, AdmittedConversationInputReceiver,
    AdmittedConversationInputSender, AdmittedConversationSendOutcome,
    AdmittedConversationSettingsSupport,
};
pub use attachment::IngressAttachment;
pub use component::SharedComponentOwned;
pub use failure::{AdmissionRefusal, AdmittedError};
pub use observation::DeliveryObservation;
pub use operation::AdmittedOperation;
pub use ownership::{AdmittedFrame, AdmittedSocket, ReservationBundle, SharedOwned};
pub use scope::{IngressWorkScope, OwnedWork};
pub use sizing::{AllocationPlan, ClassBounds, IngressClass};
pub use watch::{AdmittedWatchReceiver, AdmittedWatchSender};

/// One allocation authority shared by adapter and durable-consumer components.
/// Owners still enforce their independent component limits.
#[derive(Clone)]
pub struct IngressBudget(
    Arc<Semaphore>,
    Arc<BudgetPolicy>,
    Option<Arc<Semaphore>>,
    Option<custody::SharedCustody<attachment::AttachmentStorage>>,
);

struct BudgetPolicy {
    capacity: usize,
    classes: OnceLock<ClassPools>,
    initializing_classes: AtomicBool,
    bootstrap: root_storage::Bootstrap,
}

use classes::ClassPools;

/// Explicit runtime-only policy. Constructing this does not enable any caller.
#[derive(Clone)]
pub struct IngressOwnershipPolicy {
    root: IngressBudget,
    bounds: ClassBounds,
}

impl IngressOwnershipPolicy {
    pub fn new(root: IngressBudget, bounds: ClassBounds) -> crate::Result<Self> {
        Self::new_admitted(root, bounds).map_err(Into::into)
    }

    /// Construct the opt-in policy with fixed, allocation-free refusals.
    pub fn new_admitted(
        root: IngressBudget,
        bounds: ClassBounds,
    ) -> Result<Self, AdmissionRefusal> {
        if root.2.is_some() || root.3.is_some() || bounds.total_bytes() != root.1.capacity {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "class policy root",
            });
        }
        classes::install(&root, bounds)?;
        Ok(Self { root, bounds })
    }

    pub fn bounds(&self) -> ClassBounds {
        self.bounds
    }

    /// Budget identity only; a shared root does not confer effect authority.
    pub fn shares_root(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.root.0, &other.root.0)
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
            self.root.3.clone(),
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
        let bootstrap = root_storage::Bootstrap::new();
        Self(
            Arc::new(Semaphore::new(bytes)),
            Arc::new(BudgetPolicy {
                capacity: bytes,
                classes: OnceLock::new(),
                initializing_classes: AtomicBool::new(false),
                bootstrap,
            }),
            None,
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

    /// Fixed construction allowance before opt-in; included in metadata_bytes
    /// after policy installation. The process constructor installs one root.
    pub fn bootstrap_bytes(&self) -> usize {
        self.1.bootstrap.bytes()
    }

    /// Resident root, class and process storage stays charged while retained.
    pub fn metadata_bytes(&self) -> usize {
        self.1
            .classes
            .get()
            .map_or(0, |classes| classes.bounds.metadata_bytes())
    }

    #[cfg(test)]
    pub(crate) fn reserved_work_bytes(&self) -> usize {
        assert!(self.2.is_none(), "inspect the process root");
        self.capacity_bytes() - self.metadata_bytes() - self.available_bytes()
    }

    #[cfg(test)]
    pub(crate) fn assert_idle(&self) {
        assert!(self.2.is_none(), "inspect the process root");
        assert_eq!(
            self.available_bytes() + self.metadata_bytes(),
            self.capacity_bytes(),
            "only resident metadata may remain charged"
        );
    }

    fn try_reserve(&self, bytes: usize) -> Option<AggregateReservation> {
        let bytes = u32::try_from(bytes).ok()?;
        let class = match &self.2 {
            Some(pool) => Some(pool.clone().try_acquire_many_owned(bytes).ok()?),
            None => None,
        };
        let root = self.0.clone().try_acquire_many_owned(bytes).ok()?;
        Some(AggregateReservation {
            root,
            class,
            attachment: self.3.clone(),
            policy: self.1.clone(),
        })
    }
}

impl Drop for IngressBudget {
    fn drop(&mut self) {
        // The policy owns the class-pool allowance. Release this handle's
        // semaphore before dropping that policy, including on a final handle.
        drop(self.2.take());
        drop(self.3.take());
    }
}

struct AggregateReservation {
    root: OwnedSemaphorePermit,
    class: Option<OwnedSemaphorePermit>,
    attachment: Option<custody::SharedCustody<attachment::AttachmentStorage>>,
    // Keep class pools through permit release and their final physical drop.
    policy: Arc<BudgetPolicy>,
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
            attachment: self.attachment.clone(),
            policy: self.policy.clone(),
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

#[cfg(test)]
#[path = "ingress_budget/executor_storage_tests.rs"]
mod executor_storage_tests;

#[cfg(test)]
#[path = "ingress_budget/root_storage_tests.rs"]
mod root_storage_tests;
