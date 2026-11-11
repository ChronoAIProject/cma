//! Requested allocator bytes are measured separately from reservations and RSS.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use tracking_allocator::{
    AllocationGroupId, AllocationGroupToken, AllocationRegistry, AllocationTracker, Allocator,
};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
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

#[test]
fn measured_decode_peak_and_reply_lifetime_fit_the_derived_reservation() {
    AllocationRegistry::set_global_tracker(Tracker).unwrap();
    AllocationRegistry::enable_tracking();
    let pools = JsonPools::new(TREE_BYTES, SCRATCH_BYTES);
    let max_string = format!("\"{}\"", "x".repeat(4 * 1024 * 1024 - 2));
    let escaped = format!("\"{}\"", "\\n".repeat((4 * 1024 * 1024 - 2) / 2));
    let objects = format!("[{}]", vec![r#"{"a":[],"b":{}}"#; 1365].join(","));
    let wide_map = format!(
        "{{{}}}",
        (0..4095)
            .map(|n| format!("\"{n}\":0"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let wide_array = format!("[{}]", vec!["0"; 8191].join(","));
    let nested = format!("{}0{}", "[".repeat(64), "]".repeat(64));
    let duplicates = format!("{{{}}}", vec![r#""a":"\n\u0000\"""#; 4095].join(","));
    for (name, payload) in [
        ("max-string", max_string),
        ("max-escaped", escaped),
        ("small-objects", objects),
        ("wide-map", wide_map),
        ("wide-array", wide_array),
        ("nested", nested),
        ("duplicates", duplicates),
    ] {
        let payload = payload.as_bytes();
        let nodes = output_json_budget::inspect(payload).unwrap().nodes;
        let tree = tree_allowance(payload.len(), nodes).unwrap();
        let scratch = scratch_allowance(payload.len()).unwrap();
        let mut token = AllocationGroupToken::register().unwrap();
        GROUP.store(token.id().as_usize().get(), SeqCst);
        LIVE.store(0, SeqCst);
        PEAK.store(0, SeqCst);
        ALLOCATED.store(0, SeqCst);
        let guard = token.enter();
        let frame = pools.decode(payload).unwrap();
        drop(guard);
        let retained = LIVE.load(SeqCst);
        let peak = PEAK.load(SeqCst);
        assert!(retained <= tree, "{name}: retained {retained} > {tree}");
        assert!(
            peak <= tree + scratch,
            "{name}: peak {peak} > {}",
            tree + scratch
        );
        assert_eq!(pools.available(), (TREE_BYTES - tree, SCRATCH_BYTES));
        let guard = token.enter();
        let clone = frame.clone();
        let result = frame.into_result();
        drop(guard);
        assert_eq!(
            LIVE.load(SeqCst),
            retained,
            "projection must share the root"
        );
        drop(clone);
        assert_eq!(LIVE.load(SeqCst), retained);
        drop(result);
        assert_eq!(
            LIVE.load(SeqCst),
            0,
            "{name}: every parsed allocation must be released"
        );
        assert_eq!(pools.available(), (TREE_BYTES, SCRATCH_BYTES));
        eprintln!("JSON allocation {name}: encoded={} nodes={nodes} retained={retained} peak={peak} tree_allowance={tree} scratch_allowance={scratch} total_requested={}", payload.len(), ALLOCATED.load(SeqCst));
    }
    AllocationRegistry::disable_tracking();
}
