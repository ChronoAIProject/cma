//! Test-only requested allocation accounting, excluding allocator metadata/RSS.

use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Mutex, MutexGuard, Once};
use tracking_allocator::{
    AllocationGroupId, AllocationGroupToken, AllocationRegistry, AllocationTracker, Allocator,
};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
static LOCK: Mutex<()> = Mutex::new(());
static INITIALIZED: Once = Once::new();
static GROUP: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

struct Tracker;
impl AllocationTracker for Tracker {
    fn allocated(&self, _: usize, bytes: usize, _: usize, group: AllocationGroupId) {
        if group.as_usize().get() == GROUP.load(SeqCst) {
            ALLOCATED.fetch_add(bytes, SeqCst);
            let live = LIVE.fetch_add(bytes, SeqCst) + bytes;
            PEAK.fetch_max(live, SeqCst);
        }
    }

    fn deallocated(
        &self,
        _: usize,
        bytes: usize,
        _: usize,
        source: AllocationGroupId,
        _: AllocationGroupId,
    ) {
        if source.as_usize().get() == GROUP.load(SeqCst) {
            LIVE.fetch_sub(bytes, SeqCst);
        }
    }
}

pub(crate) struct AllocationProbe {
    token: AllocationGroupToken,
    _exclusive: MutexGuard<'static, ()>,
}

impl AllocationProbe {
    pub(crate) fn new() -> Self {
        let exclusive = LOCK.lock().unwrap();
        INITIALIZED.call_once(|| AllocationRegistry::set_global_tracker(Tracker).unwrap());
        let token = AllocationGroupToken::register().unwrap();
        GROUP.store(token.id().as_usize().get(), SeqCst);
        LIVE.store(0, SeqCst);
        PEAK.store(0, SeqCst);
        ALLOCATED.store(0, SeqCst);
        AllocationRegistry::enable_tracking();
        Self {
            token,
            _exclusive: exclusive,
        }
    }

    pub(crate) fn track<T>(&mut self, operation: impl FnOnce() -> T) -> T {
        let _guard = self.token.enter();
        operation()
    }

    pub(crate) fn live(&self) -> usize {
        LIVE.load(SeqCst)
    }

    pub(crate) fn peak(&self) -> usize {
        PEAK.load(SeqCst)
    }

    pub(crate) fn allocated(&self) -> usize {
        ALLOCATED.load(SeqCst)
    }
}

impl Drop for AllocationProbe {
    fn drop(&mut self) {
        AllocationRegistry::disable_tracking();
        GROUP.store(0, SeqCst);
    }
}
