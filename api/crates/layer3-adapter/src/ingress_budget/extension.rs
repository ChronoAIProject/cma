use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Mutex;

use super::{AdmissionRefusal, AggregateReservation, AllocationPlan, ReservationBundle};
use crate::ingress_budget::{IngressClass, IngressOwnershipPolicy};

#[derive(Default)]
pub(super) struct Extensions {
    nodes: Mutex<Option<Box<Node>>>,
    requested: AtomicUsize,
}

struct Node {
    storage: AggregateReservation,
    next: Option<Box<Node>>,
}

impl Extensions {
    pub(super) fn requested_bytes(&self) -> usize {
        self.requested.load(Ordering::Acquire)
    }

    pub(super) fn admit(
        &self,
        policy: &IngressOwnershipPolicy,
        class: IngressClass,
        plan: AllocationPlan,
    ) -> Result<(), AdmissionRefusal> {
        if !cfg!(any(target_os = "linux", target_vendor = "apple")) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "unsupported scope extension platform layout",
            });
        }
        // Tokio 1.53.3 releases the mutex through its semaphore's wait-list
        // lock. Rust 1.96.1 Apple locks lazily box one pthread_mutex_t. Charge
        // that backing conservatively per extension; Linux uses an inline futex.
        #[cfg(target_vendor = "apple")]
        let lock_backing = size_of::<libc::pthread_mutex_t>();
        #[cfg(not(target_vendor = "apple"))]
        let lock_backing = 0;
        let plan = AllocationPlan::checked(
            plan.phase(),
            &[plan.requested_bytes(), size_of::<Node>(), lock_backing],
        )?;
        let ReservationBundle { reservation, .. } = policy.admit(class, plan)?;
        // try_lock never constructs a waiter. Refusing a concurrent update
        // returns its entire attempted allowance before constructing a node.
        let mut nodes = self
            .nodes
            .try_lock()
            .map_err(|_| AdmissionRefusal::AtCapacity {
                phase: plan.phase(),
            })?;
        *nodes = Some(Box::new(Node {
            storage: reservation,
            next: nodes.take(),
        }));
        self.requested
            .fetch_add(plan.requested_bytes(), Ordering::Release);
        Ok(())
    }
}

impl Drop for Extensions {
    fn drop(&mut self) {
        // Free the semaphore's lazy lock before returning any node's allowance.
        let mut next = std::mem::take(&mut self.nodes).into_inner();
        // Iterative destruction bounds stack usage even after many failed RPCs.
        while let Some(node) = next {
            let (storage, tail) = pop(node);
            next = tail;
            drop(storage);
        }
    }
}

#[expect(
    clippy::boxed_local,
    reason = "free the node before returning its guard"
)]
fn pop(node: Box<Node>) -> (AggregateReservation, Option<Box<Node>>) {
    let Node { storage, next } = *node;
    // The consumed Box's allocation is freed when this function returns, before
    // its caller can release the extracted guard. A move alone defers that free.
    (storage, next)
}

#[cfg(test)]
#[path = "extension_tests.rs"]
mod tests;
