//! Requested allocator bytes are measured separately from reservations and RSS.
use super::*;
use crate::allocation_probe::AllocationProbe;

#[test]
fn isolated_accounted_json_deallocation_contract() {
    use crate::allocation_probe::deallocation;
    use crate::ingress_budget::{
        AllocationPlan, ClassBounds, IngressClass, IngressOwnershipPolicy,
    };
    if deallocation::isolated(
        "hosted::retained_json::allocation_tests::isolated_accounted_json_deallocation_contract",
    ) {
        return;
    }
    let root = IngressBudget::new(64 * 1024);
    let plan = |n| AllocationPlan::checked("JSON teardown fixture", &[n]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(8192), plan(8192)).unwrap(),
    )
    .unwrap();
    let pools = JsonPools::new(TREE_BYTES, SCRATCH_BYTES).with_ownership(&policy);
    drop(pools.decode(b"null").unwrap());
    let mut probe = AllocationProbe::new();
    let (frame, pause) = probe.pause_deallocation_containing(
        deallocation::arc_allocation_bytes::<IngressReservation>(),
        || pools.decode(br#"{"result":"synthetic result"}"#).unwrap(),
        |frame| match &frame.storage {
            Storage::Accounted(owner) => &*owner._storage as *const IngressReservation as usize,
            _ => panic!("expected accounted JSON"),
        },
    );
    let result = frame.clone().into_result();
    drop(frame);
    assert!(
        pause.admission_blocked_while_dropping(&policy, IngressClass::Data, move || drop(result))
    );
    root.assert_idle();
    assert_eq!(probe.live(), 0);
}

#[test]
fn measured_decode_peak_and_reply_lifetime_fit_the_derived_reservation() {
    assert!(std::mem::size_of::<Value>() <= 80);
    assert!(std::mem::size_of::<String>() <= 24);
    assert!(std::mem::size_of::<usize>() <= 8);
    let pools = JsonPools::new(TREE_BYTES, SCRATCH_BYTES);
    // On macOS the first permit release initializes a 64-byte pthread mutex
    // owned by each semaphore. Initialize coordinator infrastructure before
    // assigning frame allocation groups; it lives until the pool is dropped.
    for pool in [&pools.trees, &pools.scratch] {
        drop(pool.clone().try_acquire_owned().unwrap());
    }
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
        let mut probe = AllocationProbe::new();
        let frame = probe.track(|| pools.decode(payload).unwrap());
        let retained = probe.live();
        let peak = probe.peak();
        assert!(retained <= tree, "{name}: retained {retained} > {tree}");
        assert!(
            peak <= tree + scratch,
            "{name}: peak {peak} > {}",
            tree + scratch
        );
        assert_eq!(pools.available(), (TREE_BYTES - tree, SCRATCH_BYTES));
        let (clone, result) = probe.track(|| (frame.clone(), frame.into_result()));
        assert_eq!(probe.live(), retained, "projection must share the root");
        drop(clone);
        assert_eq!(probe.live(), retained);
        drop(result);
        assert_eq!(
            probe.live(),
            0,
            "{name}: every parsed allocation must be released"
        );
        assert_eq!(pools.available(), (TREE_BYTES, SCRATCH_BYTES));
        eprintln!("JSON allocation {name}: encoded={} nodes={nodes} retained={retained} peak={peak} tree_allowance={tree} scratch_allowance={scratch} total_requested={}", payload.len(), probe.allocated());
    }
}
