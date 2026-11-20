//! Test-only pause before one identified allocation is physically freed.

use super::AllocationProbe;
use std::{
    alloc::{GlobalAlloc, Layout},
    marker::PhantomData,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
};

static CAPTURE_BYTES: AtomicUsize = AtomicUsize::new(0);
static CAPTURE_COUNT: AtomicUsize = AtomicUsize::new(0);
static CAPTURED: [AtomicUsize; 32] = [const { AtomicUsize::new(0) }; 32];
static TARGET: AtomicUsize = AtomicUsize::new(0);
static PAUSED: AtomicBool = AtomicBool::new(false);
static RELEASE: AtomicBool = AtomicBool::new(true);

pub(super) fn allocated(address: usize, bytes: usize) {
    if bytes == CAPTURE_BYTES.load(SeqCst) && bytes != 0 {
        let index = CAPTURE_COUNT.fetch_add(1, SeqCst);
        if let Some(slot) = CAPTURED.get(index) {
            slot.store(address, SeqCst);
        }
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
    /// Select the backing containing a borrowed payload, even when another
    /// allocation has the same layout on this target.
    pub(crate) fn pause_deallocation_containing<T>(
        &mut self,
        bytes: usize,
        initialize: impl FnOnce() -> T,
        payload: impl FnOnce(&T) -> usize,
    ) -> (T, DeallocationPause<'_>) {
        self.capture_deallocation(bytes, initialize, |value| Some(payload(value)))
    }

    fn capture_deallocation<T>(
        &mut self,
        bytes: usize,
        initialize: impl FnOnce() -> T,
        payload: impl FnOnce(&T) -> Option<usize>,
    ) -> (T, DeallocationPause<'_>) {
        struct Capture;
        impl Drop for Capture {
            fn drop(&mut self) {
                CAPTURE_BYTES.store(0, SeqCst);
            }
        }
        CAPTURE_COUNT.store(0, SeqCst);
        CAPTURE_BYTES.store(bytes, SeqCst);
        let capture = Capture;
        let value = self.track(initialize);
        drop(capture);
        let count = CAPTURE_COUNT.load(SeqCst);
        assert!(
            count <= CAPTURED.len(),
            "allocation capture capacity exceeded"
        );
        let payload = payload(&value);
        let mut candidates = CAPTURED[..count]
            .iter()
            .map(|slot| slot.load(SeqCst))
            .filter(|address| payload.is_none_or(|payload| payload.wrapping_sub(*address) < bytes));
        let target = candidates
            .next()
            .expect("selected backing was not allocated");
        assert!(
            candidates.next().is_none(),
            "select one exact backing allocation"
        );
        RELEASE.store(false, SeqCst);
        PAUSED.store(false, SeqCst);
        TARGET.store(target, SeqCst);
        (
            value,
            DeallocationPause {
                _probe: PhantomData,
            },
        )
    }
}

impl DeallocationPause<'_> {
    /// Inspect the remaining charge after child fields drop but before the
    /// selected outer allocation is freed. Children may release their own plans.
    pub(crate) fn inspect_while_dropping<T>(
        self,
        drop_owner: impl FnOnce() + Send + 'static,
        inspect: impl FnOnce() -> T,
    ) -> T {
        let child = std::thread::spawn(drop_owner);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !PAUSED.load(SeqCst) && std::time::Instant::now() < end {
            std::thread::yield_now();
        }
        let paused = PAUSED.load(SeqCst);
        assert!(paused, "selected backing did not reach its deallocator");
        let result = inspect();
        drop(self);
        child.join().unwrap();
        result
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
