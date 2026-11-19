use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::{AdmissionRefusal, AllocationPlan, ClassBounds, IngressBudget};

pub(super) struct ClassPools {
    pub(super) bounds: ClassBounds,
    pub(super) data: Arc<Semaphore>,
    pub(super) transformation: Arc<Semaphore>,
    pub(super) control: Arc<Semaphore>,
    pub(super) active_attachments: AtomicUsize,
    // All three Arc allocations and their lock backing are freed first. Every
    // class handle/permit retains BudgetPolicy until its own semaphore drop.
    _storage: OwnedSemaphorePermit,
}

pub(super) fn storage_bytes() -> Result<usize, AdmissionRefusal> {
    if !cfg!(any(target_os = "linux", target_vendor = "apple")) {
        return Err(AdmissionRefusal::InvalidPlan {
            phase: "unsupported class pool layout",
        });
    }
    // Tokio 1.53.3 releases permits through a std mutex.
    let lock_backing = super::root_storage::lazy_lock_bytes();
    let semaphore = AllocationPlan::checked(
        "class semaphore storage",
        &[
            size_of::<Semaphore>(),
            2 * size_of::<usize>(),
            align_of::<Semaphore>(),
            lock_backing,
        ],
    )?
    .requested_bytes();
    Ok(AllocationPlan::checked("class pools", &[semaphore; 3])?.requested_bytes())
}

pub(super) fn install(root: &IngressBudget, bounds: ClassBounds) -> Result<(), AdmissionRefusal> {
    if root.1.classes.get().is_none() {
        let _initializing = Initialization::claim(&root.1.initializing_classes)?;
        if root.1.classes.get().is_none() {
            let mut storage = root
                .0
                .clone()
                .try_acquire_many_owned(bounds.metadata_bytes() as u32)
                .map_err(|_| AdmissionRefusal::AtCapacity {
                    phase: "class pools",
                })?;
            let bootstrap = storage
                .split(root.bootstrap_bytes())
                .expect("resident bounds include the fixed root bootstrap");
            let classes = ClassPools {
                bounds,
                data: Arc::new(Semaphore::new(bounds.data_bytes())),
                transformation: Arc::new(Semaphore::new(bounds.transformation_bytes())),
                control: Arc::new(Semaphore::new(bounds.control_bytes())),
                active_attachments: AtomicUsize::new(0),
                _storage: storage,
            };
            assert!(
                root.1.classes.set(classes).is_ok(),
                "exclusive class initialization"
            );
            // This root cannot reclaim its own allocation while it still
            // exists. Debit its fixed bootstrap once, after successful install;
            // only the separate class allocations have returnable permits.
            bootstrap.forget();
        }
    }
    if root.1.classes.get().expect("installed class pools").bounds != bounds {
        return Err(AdmissionRefusal::InvalidPlan {
            phase: "conflicting class bounds",
        });
    }
    Ok(())
}

struct Initialization<'a>(&'a AtomicBool);

impl<'a> Initialization<'a> {
    fn claim(initializing: &'a AtomicBool) -> Result<Self, AdmissionRefusal> {
        // Concurrent temporary pools could consume the protected root floors.
        // Claim the only initializer before reserving or allocating anything.
        initializing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| AdmissionRefusal::AtCapacity {
                phase: "class pool initialization",
            })?;
        Ok(Self(initializing))
    }
}

impl Drop for Initialization<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[cfg(test)]
#[path = "classes_tests.rs"]
mod tests;
