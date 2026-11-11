use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::hosted::retained_json::JsonPools;
use crate::ingress_budget::{IngressBudget, IngressReservation};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Semaphore;

fn value(session: &str, goal: Value) -> Value {
    json!({"params":{"sessionId":session,"update":{"sessionUpdate":"session_info_update","_meta":{"goal":goal}}}})
}

fn frame(pools: &JsonPools, value: Value, admitted: bool) -> JsonFrame {
    if admitted {
        pools.decode(&serde_json::to_vec(&value).unwrap()).unwrap()
    } else {
        JsonFrame::legacy(value)
    }
}

#[test]
fn exact_identity_and_sticky_goal_safety_match_with_policy_enabled_or_disabled() {
    for admitted in [false, true] {
        let pools = JsonPools::new(128 * 1024, 128 * 1024);
        let mut observed = None;
        ContextGoal::observe(
            &mut observed,
            &frame(&pools, value("s", Value::Null), admitted),
        );
        assert!(observed.as_ref().unwrap().is_safe_for("s"));
        assert!(!observed.as_ref().unwrap().is_safe_for("other"));
        ContextGoal::observe(
            &mut observed,
            &frame(&pools, value("other", json!({"status":"active"})), admitted),
        );
        assert!(observed.as_ref().unwrap().is_safe_for("s"));
        ContextGoal::observe(
            &mut observed,
            &frame(&pools, value("s", json!({"status":"active"})), admitted),
        );
        assert!(!observed.as_ref().unwrap().is_safe_for("s"));
        for goal in [json!({"status":"paused"}), Value::Null, json!(false)] {
            ContextGoal::observe(&mut observed, &frame(&pools, value("s", goal), admitted));
            assert!(!observed.as_ref().unwrap().is_safe_for("s"));
        }
        drop(observed);
        assert_eq!(pools.available(), (128 * 1024, 128 * 1024));
    }
}

#[test]
fn malformed_or_unrelated_observations_do_not_retain_a_frame() {
    let pools = JsonPools::new(128 * 1024, 128 * 1024);
    for admitted in [false, true] {
        for value in [
            Value::Null,
            json!({"params":{"sessionId":false,"update":{"sessionUpdate":"session_info_update","_meta":{"goal":null}}}}),
            json!({"params":{"sessionId":"s"}}),
            json!({"params":{"sessionId":"s","update":{"sessionUpdate":"usage_update","_meta":{"goal":null}}}}),
            json!({"params":{"sessionId":"s","update":{"sessionUpdate":"session_info_update","_meta":{}}}}),
        ] {
            let mut observed = None;
            ContextGoal::observe(&mut observed, &frame(&pools, value, admitted));
            assert!(observed.is_none());
            assert_eq!(pools.available(), (128 * 1024, 128 * 1024));
        }
    }
}

#[test]
fn admitted_observation_and_readiness_add_no_allocation_and_hold_full_frame_storage() {
    const TREE: usize = 32 * 1024 * 1024;
    const SCRATCH: usize = 13 * 1024 * 1024;
    const TOTAL: usize = 128 * 1024 * 1024;
    let aggregate = IngressBudget::new(TOTAL);
    let pools = JsonPools::new(TREE, SCRATCH).with_aggregate(Some(aggregate.clone()));
    drop(pools.decode(b"{}").unwrap()); // Initialize pool bookkeeping outside the sample.
    let session = "s".repeat(1024);
    let mut input = value(&session, Value::Null);
    input["unused"] = json!("x".repeat(3_000_000));
    let encoded = serde_json::to_vec(&input).unwrap();
    let mut probe = AllocationProbe::new();
    let frame = probe.track(|| pools.decode(&encoded).unwrap());
    let (retained, peak, allocated) = (probe.live(), probe.peak(), probe.allocated());
    let reserved = TREE - pools.available().0;
    let mut observed = None;
    probe.track(|| {
        ContextGoal::observe(&mut observed, &frame);
        for _ in 0..100 {
            assert!(observed.as_ref().unwrap().is_safe_for(&session));
            assert!(!observed.as_ref().unwrap().is_safe_for("foreign"));
        }
    });
    assert_eq!(
        (probe.live(), probe.peak(), probe.allocated()),
        (retained, peak, allocated)
    );
    drop(frame);
    assert_eq!(
        probe.live(),
        retained,
        "unused siblings stay owned and charged"
    );
    assert_eq!(pools.available(), (TREE - reserved, SCRATCH));
    assert_eq!(aggregate.available_bytes(), TOTAL - reserved);
    drop(observed);
    assert_eq!(probe.live(), 0);
    assert_eq!(pools.available(), (TREE, SCRATCH));
    assert_eq!(aggregate.available_bytes(), TOTAL);
    eprintln!("Context observation allocation: encoded={} reserved={reserved} retained={retained} peak={peak} observation_added=0 final=0", encoded.len());
}

#[test]
fn held_observation_preserves_local_and_shared_admission_until_drop() {
    let payload = serde_json::to_vec(&value("s", Value::Null)).unwrap();
    let sizing = JsonPools::new(128 * 1024, 128 * 1024);
    let sample = sizing.decode(&payload).unwrap();
    let charge = 128 * 1024 - sizing.available().0;
    drop(sample);
    for shared in [false, true] {
        let aggregate = IngressBudget::new(128 * 1024);
        let pools = JsonPools::new(if shared { 128 * 1024 } else { charge }, 128 * 1024)
            .with_aggregate(shared.then(|| aggregate.clone()));
        let mut observed = None;
        ContextGoal::observe(&mut observed, &pools.decode(&payload).unwrap());
        let component = Arc::new(Semaphore::new(128 * 1024));
        let blocker = shared.then(|| {
            IngressReservation::try_acquire(
                &component,
                Some(&aggregate),
                aggregate.available_bytes(),
            )
            .unwrap()
        });
        let held = (pools.available(), aggregate.available_bytes());
        assert!(matches!(
            pools.decode(&payload),
            Err(crate::AdapterError::AtCapacity(_))
        ));
        assert_eq!((pools.available(), aggregate.available_bytes()), held);
        assert!(observed.as_ref().unwrap().is_safe_for("s"));
        drop(blocker);
        drop(observed);
        drop(pools.decode(&payload).unwrap());
        assert_eq!(aggregate.available_bytes(), 128 * 1024);
    }
}
