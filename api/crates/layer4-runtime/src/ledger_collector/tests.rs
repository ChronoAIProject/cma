use super::*;
use std::{
    future::Future,
    task::{Context, Poll, Waker},
};
#[path = "lifecycle_harness.rs"]
mod lifecycle_harness;
#[path = "model.rs"]
mod model;
use lifecycle_harness::until_complete;
use model::{digest, Call, Model};
#[path = "archive_contract_tests.rs"]
mod archive_contract_tests;
#[path = "checkpoint_tests.rs"]
mod checkpoint_tests;
#[path = "pending_discovery_tests.rs"]
mod pending_discovery_tests;
#[path = "window_tests.rs"]
mod window_tests;

fn run<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    let waker = Waker::noop();
    match future.as_mut().poll(&mut Context::from_waker(waker)) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("the model unexpectedly awaited external work"),
    }
}
fn enabled() -> Admission {
    Admission { new_work: true }
}
fn selected(model: &Model) {
    assert_eq!(run(step(enabled(), model)), Ok(Step::PagePinned));
    assert_eq!(run(step(enabled(), model)), Ok(Step::ReferenceRecorded));
    assert_eq!(run(step(enabled(), model)), Ok(Step::EvidenceSelected));
}

#[test]
fn closed_admission_has_no_effects_but_reconciles_original_pending_work() {
    let m = Model::new(
        SourceFormat::V1,
        &[(1, Decision::SettledDeleted), (2, Decision::SettledDeleted)],
    );
    assert_eq!(run(step(Admission::default(), &m)), Ok(Step::Disabled));
    assert_eq!(m.state.lock().unwrap().enumerations, 0);
    selected(&m);
    let original = m
        .state
        .lock()
        .unwrap()
        .checkpoint
        .pending
        .clone()
        .unwrap()
        .intent;
    for _ in 0..8 {
        if run(step(Admission::default(), &m)).unwrap() == Step::Disabled {
            break;
        }
    }
    let s = m.state.lock().unwrap();
    assert_eq!(s.selected, [original.operation]);
    assert_eq!(s.mutations, 1);
    assert!(s.checkpoint.pending.is_none());
    assert!(s.groups.contains_key(&digest(2)));
}

#[test]
fn lost_replies_at_each_boundary_keep_one_operation_across_restart_and_takeover() {
    for format in [SourceFormat::V1, SourceFormat::V2] {
        for fault in [
            Call::Evidence,
            Call::Retain,
            Call::Reference,
            Call::Source,
            Call::Cold,
            Call::Select,
            Call::Dispatch,
            Call::Record,
            Call::Settle,
        ] {
            let mut m = Model::new(format, &[(1, Decision::SettledDeleted)]);
            m.state.lock().unwrap().fail_after = Some(fault);
            let mut failed = false;
            let mut completed = false;
            for _ in 0..24 {
                match run(step(enabled(), &m)) {
                    Err(Refusal::UnknownOutcome) => {
                        assert!(!failed);
                        failed = true;
                        {
                            let mut s = m.state.lock().unwrap();
                            s.checkpoint.fence += 1;
                            s.checkpoint.revision += 1;
                        }
                        m = m.reopened();
                    }
                    Ok(Step::InventoryComplete) => {
                        completed = true;
                        break;
                    }
                    Ok(_) => (),
                    other => panic!("unexpected result {other:?}"),
                }
            }
            let s = m.state.lock().unwrap();
            assert!(failed && completed, "{format:?} {fault:?}");
            assert_eq!(s.mutations, 1);
            assert_eq!(s.selected.len(), 1);
            assert_eq!(s.evidence.len(), 1);
            assert_eq!(s.cold.len(), 1);
            assert_eq!(s.next_operation, 2);
            assert!(s.checkpoint.pending.is_none());
        }
    }
}

#[test]
fn cancellation_after_source_commit_keeps_admitted_guard_and_original_retry() {
    let m = Model::new(SourceFormat::V2, &[(1, Decision::SettledDeleted)]);
    selected(&m);
    m.state.lock().unwrap().suspend_dispatch = true;
    let mut future = Box::pin(step(enabled(), &m));
    let waker = Waker::noop();
    assert!(future
        .as_mut()
        .poll(&mut Context::from_waker(waker))
        .is_pending());
    drop(future);
    assert_eq!(m.holds.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(run(step(enabled(), &m)), Err(Refusal::Conflict));
    {
        let mut s = m.state.lock().unwrap();
        assert_eq!(s.mutations, 1);
        assert!(s.checkpoint.pending.as_ref().unwrap().completion.is_none());
        s.in_flight.take();
        s.suspend_dispatch = false;
    }
    assert_eq!(m.holds.load(std::sync::atomic::Ordering::SeqCst), 0);
    run(until_complete(&m.reopened(), 16)).unwrap();
    assert_eq!(m.state.lock().unwrap().mutations, 1);
}

#[test]
fn missing_archive_bad_receipts_and_changed_authority_never_advance_or_reidentify() {
    for error in [Refusal::ArchiveUnavailable, Refusal::ArchiveCapacity] {
        let m = Model::new(SourceFormat::V1, &[(1, Decision::SettledDeleted)]);
        selected(&m);
        let before = run(m.load()).unwrap();
        m.state.lock().unwrap().unavailable = Some(error);
        assert_eq!(run(step(enabled(), &m)), Err(error));
        assert_eq!(run(m.load()).unwrap(), before);
        assert_eq!(m.state.lock().unwrap().mutations, 0);
    }
    let m = Model::new(SourceFormat::V2, &[(1, Decision::SettledDeleted)]);
    selected(&m);
    m.state.lock().unwrap().wrong_authority = true;
    assert_eq!(run(step(enabled(), &m)), Err(Refusal::StaleAuthority));
    m.state.lock().unwrap().wrong_authority = false;
    assert_eq!(run(step(enabled(), &m)), Ok(Step::OriginalSubmitted));
    let before = run(m.load()).unwrap();
    m.state.lock().unwrap().corrupt_receipt = true;
    assert_eq!(run(step(enabled(), &m)), Err(Refusal::InvalidEvidence));
    assert_eq!(run(m.load()).unwrap(), before);
    m.state.lock().unwrap().corrupt_receipt = false;
    m.state.lock().unwrap().evidence.clear();
    assert_eq!(run(step(enabled(), &m)), Err(Refusal::InvalidEvidence));
    assert_eq!(run(m.load()).unwrap(), before);
}

#[test]
fn authority_change_between_observation_and_dispatch_holds_without_resetting_pending() {
    let m = Model::new(SourceFormat::V2, &[(1, Decision::SettledDeleted)]);
    selected(&m);
    let original = run(m.load()).unwrap();
    m.state.lock().unwrap().source_revision += 1;
    assert_eq!(run(step(enabled(), &m)), Err(Refusal::UnknownOutcome));
    assert_eq!(run(m.load()).unwrap(), original);
    assert_eq!(m.state.lock().unwrap().mutations, 0);
}

#[test]
fn durable_fair_position_passes_live_first_page_and_new_earlier_work() {
    for format in [SourceFormat::V1, SourceFormat::V2] {
        let mut groups = vec![(1, Decision::RetainLive), (2, Decision::RetainUnknown)];
        groups.extend((1000..1130).map(|i| (i, Decision::SettledDeleted)));
        let m = Model::new(format, &groups);
        let mut complete = false;
        let mut injected = 10;
        for _ in 0..1500 {
            let result = run(step(enabled(), &m)).unwrap();
            if result == Step::GroupSettled && m.state.lock().unwrap().mutations > 0 {
                let mut s = m.state.lock().unwrap();
                // Continuous changes behind the fair position cannot restart
                // enumeration from the first key and starve the tail.
                s.groups.insert(digest(injected), Decision::RetainLive);
                s.source_revision += 1;
                injected += 1;
            }
            if result == Step::InventoryComplete {
                complete = true;
                break;
            }
        }
        let s = m.state.lock().unwrap();
        assert!(complete);
        assert_eq!(s.mutations, 130);
        assert!(!s.groups.values().any(|d| *d == Decision::SettledDeleted));
        assert!(s.groups.contains_key(&digest(1)) && s.groups.contains_key(&digest(2)));
        assert!(s.enumerations <= 132);
    }
}

#[test]
fn competing_workers_cannot_select_two_operations_for_one_checkpoint() {
    let m = Model::new(SourceFormat::V1, &[(1, Decision::SettledDeleted)]);
    run(step(enabled(), &m)).unwrap();
    assert_eq!(run(step(enabled(), &m)), Ok(Step::ReferenceRecorded));
    let checkpoint = run(m.load()).unwrap();
    let a = m.clone();
    let b = m.clone();
    let checkpoint_b = checkpoint.clone();
    let first = std::thread::spawn(move || run(a.select(&checkpoint)));
    let second = std::thread::spawn(move || run(b.select(&checkpoint_b)));
    let outcomes = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| **r == Err(Refusal::Conflict))
            .count(),
        1
    );
    assert_eq!(m.state.lock().unwrap().selected.len(), 1);
}

#[test]
fn both_format_model_drivers_exercise_the_lifecycle_harness_past_legacy_bounds() {
    for format in [SourceFormat::V1, SourceFormat::V2] {
        let model = Model::new(format, &[]);
        let metrics = run(lifecycle_harness::lifetime(model.clone(), 1400)).unwrap();
        assert_eq!(metrics.retained_groups, 1400);
        assert_eq!(metrics.hot_progress_slots, 0);
        let s = model.state.lock().unwrap();
        assert_eq!(s.retired_operations, 8400);
        assert_eq!(s.retired_authority, 4200);
        // These are model counters. Actual V1 bytes/V2 pages require A's drivers.
    }
}

#[test]
fn lifecycle_harness_preserves_live_and_unresolved_baselines() {
    for format in [SourceFormat::V1, SourceFormat::V2] {
        let model = Model::new(
            format,
            &[
                (1, Decision::RetainLive),
                (u64::MAX, Decision::RetainUnknown),
            ],
        );
        let metrics = run(lifecycle_harness::lifetime(model.clone(), 1_400)).unwrap();
        assert_eq!(metrics.active_operations, 12);
        assert_eq!(metrics.active_authority, 6);
        assert_eq!(metrics.reclaimed_operations, 8_400);
        assert_eq!(metrics.reclaimed_authority, 4_200);
        assert_eq!(model.state.lock().unwrap().groups.len(), 2);
    }
}

#[test]
fn lifecycle_harness_does_not_qualify_an_insufficient_campaign() {
    assert_eq!(
        run(lifecycle_harness::lifetime(
            Model::new(SourceFormat::V2, &[]),
            1
        )),
        Err(Refusal::InvalidEvidence)
    );
}

#[test]
fn completed_sweep_wakes_on_changed_source_and_revisits_earlier_keys() {
    let m = Model::new(SourceFormat::V1, &[(100, Decision::SettledDeleted)]);
    run(until_complete(&m, 16)).unwrap();
    assert_eq!(run(step(enabled(), &m)), Ok(Step::InventoryComplete));
    m.add_cycle(1);
    assert_eq!(run(step(enabled(), &m)), Ok(Step::RevisionRefreshed));
    assert!(run(m.load()).unwrap().after_group.is_none());
    run(until_complete(&m, 16)).unwrap();
    assert_eq!(m.state.lock().unwrap().mutations, 2);
}

#[test]
fn empty_page_with_continuation_is_not_inventory_completion() {
    let m = Model::new(SourceFormat::V2, &[]);
    run(step(enabled(), &m)).unwrap();
    {
        let mut s = m.state.lock().unwrap();
        let page = s.checkpoint.page.as_mut().unwrap();
        page.complete = false;
        page.next_cursor = Some("next-bounded-page".into());
    }
    assert_eq!(run(step(enabled(), &m)), Ok(Step::PageAdvanced));
    assert!(!run(m.load()).unwrap().complete);
    assert_eq!(m.state.lock().unwrap().mutations, 0);
}

#[test]
fn malformed_progress_or_forged_reference_has_zero_source_effects() {
    for case in 0..6 {
        let m = Model::new(SourceFormat::V2, &[(1, Decision::SettledDeleted)]);
        selected(&m);
        {
            let mut s = m.state.lock().unwrap();
            let c = &mut s.checkpoint;
            match case {
                0 => c.binding.epoch = 0,
                1 => c.page.as_mut().unwrap().groups = vec![digest(1); 65],
                2 => c.page.as_mut().unwrap().source_revision += 1,
                3 => c.page.as_mut().unwrap().next_cursor = Some("contradictory-end".into()),
                4 => c.pending.as_mut().unwrap().intent.binding.sha256 = digest(42),
                _ => {
                    let pending = c.pending.as_mut().unwrap();
                    pending.reference.as_mut().unwrap().artifacts_sha256 = hex(&digest(42));
                }
            }
        }
        assert_eq!(
            run(step(enabled(), &m)),
            Err(Refusal::InvalidEvidence),
            "case {case}"
        );
        assert_eq!(m.state.lock().unwrap().mutations, 0);
    }
}
