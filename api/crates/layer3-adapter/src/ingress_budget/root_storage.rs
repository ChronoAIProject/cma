use super::{sizing::shared_allocation_bytes, BudgetPolicy, Semaphore};

/// The closed root constructor has two fixed Arcs and one possible lazy lock.
/// Before opt-in this finite provision is separate from legacy payload tokens;
/// installing classes transfers it into the same root's resident allowance.
pub(super) struct Bootstrap {
    bytes: usize,
}

impl Bootstrap {
    pub(super) fn new() -> Self {
        Self {
            bytes: shared_allocation_bytes::<Semaphore>()
                + shared_allocation_bytes::<BudgetPolicy>()
                + lazy_lock_bytes(),
        }
    }

    pub(super) fn bytes(&self) -> usize {
        self.bytes
    }
}

// The supported opt-in targets are checked by classes::storage_bytes.
// Rust 1.96.1 Apple mutexes allocate pthread backing lazily; Linux is inline.
pub(super) fn lazy_lock_bytes() -> usize {
    #[cfg(target_vendor = "apple")]
    let bytes = size_of::<libc::pthread_mutex_t>();
    #[cfg(not(target_vendor = "apple"))]
    let bytes = 0;
    bytes
}
