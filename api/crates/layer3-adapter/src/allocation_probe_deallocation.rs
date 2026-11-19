//! Test-only pause before one identified allocation is physically freed.

use super::AllocationProbe;
use crate::ingress_budget::{AllocationPlan, IngressClass, IngressOwnershipPolicy};
use std::{
    alloc::{GlobalAlloc, Layout},
    marker::PhantomData,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
};

static CAPTURE_BYTES: AtomicUsize = AtomicUsize::new(0);
static CAPTURE_COUNT: AtomicUsize = AtomicUsize::new(0);
static CAPTURED: AtomicUsize = AtomicUsize::new(0);
static TARGET: AtomicUsize = AtomicUsize::new(0);
static PAUSED: AtomicBool = AtomicBool::new(false);
static RELEASE: AtomicBool = AtomicBool::new(true);

pub(super) fn allocated(address: usize, bytes: usize) {
    if bytes == CAPTURE_BYTES.load(SeqCst) && bytes != 0 {
        CAPTURED.store(address, SeqCst);
        CAPTURE_COUNT.fetch_add(1, SeqCst);
    }
}

struct PausingAllocator;
#[global_allocator]
static ALLOCATOR: PausingAllocator = PausingAllocator;

unsafe impl GlobalAlloc for PausingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { super::TRACKED_ALLOCATOR.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        unsafe { super::TRACKED_ALLOCATOR.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        unsafe { super::TRACKED_ALLOCATOR.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if TARGET
            .compare_exchange(ptr as usize, 0, SeqCst, SeqCst)
            .is_ok()
        {
            PAUSED.store(true, SeqCst);
            while !RELEASE.load(SeqCst) {
                std::thread::yield_now();
            }
        }
        unsafe { super::TRACKED_ALLOCATOR.dealloc(ptr, layout) }
    }
}

pub(crate) fn arc_allocation_bytes<T>() -> usize {
    Layout::new::<[usize; 2]>()
        .extend(Layout::new::<T>())
        .unwrap()
        .0
        .pad_to_align()
        .size()
}

pub(crate) struct DeallocationPause<'a> {
    _probe: PhantomData<&'a mut AllocationProbe>,
}

impl AllocationProbe {
    pub(crate) fn pause_deallocation<T>(
        &mut self,
        bytes: usize,
        initialize: impl FnOnce() -> T,
    ) -> (T, DeallocationPause<'_>) {
        struct Capture;
        impl Drop for Capture {
            fn drop(&mut self) {
                CAPTURE_BYTES.store(0, SeqCst);
            }
        }
        CAPTURE_COUNT.store(0, SeqCst);
        CAPTURED.store(0, SeqCst);
        CAPTURE_BYTES.store(bytes, SeqCst);
        let capture = Capture;
        let value = self.track(initialize);
        drop(capture);
        assert_eq!(
            CAPTURE_COUNT.load(SeqCst),
            1,
            "select one exact backing allocation"
        );
        RELEASE.store(false, SeqCst);
        PAUSED.store(false, SeqCst);
        TARGET.store(CAPTURED.load(SeqCst), SeqCst);
        (
            value,
            DeallocationPause {
                _probe: PhantomData,
            },
        )
    }
}

impl DeallocationPause<'_> {
    pub(crate) fn admission_blocked_while_dropping(
        self,
        policy: &IngressOwnershipPolicy,
        class: IngressClass,
        drop_owner: impl FnOnce() + Send + 'static,
    ) -> bool {
        let available = policy.budget(class).available_bytes();
        let pressure = policy
            .admit(
                class,
                AllocationPlan::checked("deallocation sentinel pressure", &[available]).unwrap(),
            )
            .unwrap();
        let child = std::thread::spawn(drop_owner);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !PAUSED.load(SeqCst) && std::time::Instant::now() < end {
            std::thread::yield_now();
        }
        let paused = PAUSED.load(SeqCst);
        let admitted = policy.admit(
            class,
            AllocationPlan::checked("deallocation sentinel", &[1]).unwrap(),
        );
        let blocked = admitted.is_err();
        drop(self);
        child.join().unwrap();
        drop((admitted, pressure));
        assert!(paused, "selected backing did not reach its deallocator");
        blocked
    }
}

impl Drop for DeallocationPause<'_> {
    fn drop(&mut self) {
        RELEASE.store(true, SeqCst);
        TARGET.store(0, SeqCst);
    }
}

/// Return true in the parent after verifying one isolated child test.
pub(crate) fn isolated(test: &str) -> bool {
    if std::env::var("CMA_DEALLOCATION_TEST_CHILD").as_deref() == Ok(test) {
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env("CMA_DEALLOCATION_TEST_CHILD", test)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    true
}
