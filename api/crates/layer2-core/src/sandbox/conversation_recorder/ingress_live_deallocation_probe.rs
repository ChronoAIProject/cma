use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
};

thread_local! { static CAPTURE: Cell<bool> = const { Cell::new(false) }; }
static COUNT: AtomicUsize = AtomicUsize::new(0);
static ADDRESSES: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];
static SIZES: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];
static LIVE: [AtomicBool; 16] = [const { AtomicBool::new(false) }; 16];
static TARGET: AtomicUsize = AtomicUsize::new(0);
static PAUSED: AtomicBool = AtomicBool::new(false);
static RELEASE: AtomicBool = AtomicBool::new(true);

struct PausingAllocator;
#[global_allocator]
static ALLOCATOR: PausingAllocator = PausingAllocator;

fn allocated(address: *mut u8, bytes: usize) {
    if !address.is_null() && CAPTURE.try_with(Cell::get).unwrap_or(false) {
        let index = COUNT.fetch_add(1, SeqCst);
        if let Some(slot) = ADDRESSES.get(index) {
            slot.store(address as usize, SeqCst);
            SIZES[index].store(bytes, SeqCst);
            LIVE[index].store(true, SeqCst);
        }
    }
}

unsafe impl GlobalAlloc for PausingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        allocated(pointer, layout.size());
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        allocated(pointer, layout.size());
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if TARGET
            .compare_exchange(pointer as usize, 0, SeqCst, SeqCst)
            .is_ok()
        {
            PAUSED.store(true, SeqCst);
            while !RELEASE.load(SeqCst) {
                std::thread::yield_now();
            }
        }
        unsafe { System.dealloc(pointer, layout) };
        // Observe the real free before removing this requested layout from live.
        for (index, address) in ADDRESSES.iter().enumerate() {
            if address.load(SeqCst) == pointer as usize {
                LIVE[index].store(false, SeqCst);
            }
        }
    }
}

pub(super) fn capture<T>(initialize: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            CAPTURE.set(false);
        }
    }
    COUNT.store(0, SeqCst);
    for index in 0..ADDRESSES.len() {
        ADDRESSES[index].store(0, SeqCst);
        SIZES[index].store(0, SeqCst);
        LIVE[index].store(false, SeqCst);
    }
    CAPTURE.set(true);
    let reset = Reset;
    let value = initialize();
    drop(reset);
    assert!(COUNT.load(SeqCst) <= ADDRESSES.len());
    value
}

pub(super) fn address_of_size(bytes: usize) -> usize {
    let mut matches = (0..COUNT.load(SeqCst))
        .filter(|index| SIZES[*index].load(SeqCst) == bytes && LIVE[*index].load(SeqCst))
        .map(|index| ADDRESSES[index].load(SeqCst));
    let address = matches.next().expect("the exact backing was not captured");
    assert!(
        matches.next().is_none(),
        "backing layout must identify one allocation"
    );
    address
}

pub(super) fn live_bytes() -> usize {
    (0..COUNT.load(SeqCst))
        .filter(|index| LIVE[*index].load(SeqCst))
        .map(|index| SIZES[index].load(SeqCst))
        .sum()
}

pub(super) fn live_layouts() -> Vec<usize> {
    (0..COUNT.load(SeqCst))
        .filter(|index| LIVE[*index].load(SeqCst))
        .map(|index| SIZES[index].load(SeqCst))
        .collect()
}

pub(super) fn pause<T>(
    address: usize,
    release_owner: impl FnOnce() + Send + 'static,
    inspect: impl FnOnce() -> T,
) -> T {
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            RELEASE.store(true, SeqCst);
            TARGET.store(0, SeqCst);
        }
    }
    PAUSED.store(false, SeqCst);
    RELEASE.store(false, SeqCst);
    TARGET.store(address, SeqCst);
    let release = Release;
    let worker = std::thread::spawn(release_owner);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !PAUSED.load(SeqCst) && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert!(
        PAUSED.load(SeqCst),
        "exact backing did not reach its physical free"
    );
    let result = inspect();
    drop(release);
    worker.join().unwrap();
    result
}

pub(super) fn isolated(test: &str) -> bool {
    if std::env::var("CMA_LIVE_FREE_CHILD").as_deref() == Ok(test) {
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env("CMA_LIVE_FREE_CHILD", test)
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
