use super::*;
use crate::binding_diagnostics::LocalSource;
use layer2_core::binding_diagnostics::{
    AdapterFailureClass, HoldPhase, HoldReason, MAX_HOLDS_PER_REPORTER,
};

#[tokio::test(start_paused = true)]
async fn binding_diagnostics_are_opt_in_and_exclude_initial_in_flight_work() {
    let (_, reconciler) = setup(0, Duration::ZERO).await;
    let row = sandbox(0);
    reconciler
        .binding_holds
        .begin(&row, &[])
        .unwrap()
        .unavailable();
    assert!(reconciler.binding_holds.sample().holds.is_empty());
    assert!(reconciler.binding_is_held(&row.id));
    let (_, observed) = setup(0, Duration::ZERO).await;
    let sampler = observed.binding_diagnostics();
    let attempt = observed.binding_holds.begin(&row, &[]).unwrap();
    assert!(sampler.sample().holds.is_empty());
    assert!(observed.binding_is_held(&row.id));
    attempt.complete();
    assert!(sampler.sample().holds.is_empty());
    assert!(!observed.binding_is_held(&row.id));
}

#[tokio::test(start_paused = true)]
async fn observation_retains_episode_through_retry_without_granting_retry() {
    let (_, reconciler) = setup(0, Duration::ZERO).await;
    let sampler = reconciler.binding_diagnostics();
    let row = sandbox(0);
    reconciler
        .binding_holds
        .begin(&row, &[])
        .unwrap()
        .unavailable();
    let first = sampler.sample().holds.remove(0);
    assert_eq!(first.sandbox_id, row.id);
    assert_eq!(first.owner, row.owner);
    assert_eq!(first.attempts, 1);
    assert_eq!(first.reason, HoldReason::Unavailable);
    assert_eq!(first.phase, HoldPhase::Deferred);
    assert_eq!(
        first.adapter_error_class,
        Some(AdapterFailureClass::Unavailable)
    );
    assert!(first.retry_not_before.is_some());
    for _ in 0..5 {
        assert_eq!(sampler.sample().holds[0].first_held_at, first.first_held_at);
        assert!(reconciler.binding_holds.begin(&row, &[]).is_none());
    }
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(reconciler.binding_holds.begin(&row, &[]).is_none());
    tokio::time::advance(Duration::from_secs(1)).await;
    let attempt = reconciler.binding_holds.begin(&row, &[]).unwrap();
    let running = sampler.sample().holds.remove(0);
    assert_eq!(running.phase, HoldPhase::Retrying);
    assert_eq!(running.attempts, 2);
    assert_eq!(running.first_held_at, first.first_held_at);
    assert!(running.retry_not_before.is_none());
    drop(attempt);
    let cancelled = sampler.sample().holds.remove(0);
    assert_eq!(cancelled.reason, HoldReason::Cancelled);
    assert_eq!(cancelled.phase, HoldPhase::Deferred);
    assert_eq!(cancelled.first_held_at, first.first_held_at);
    assert_eq!(cancelled.adapter_error_class, first.adapter_error_class);
    tokio::time::advance(Duration::from_secs(60)).await;
    reconciler
        .binding_holds
        .begin(&row, &[])
        .unwrap()
        .complete();
    assert!(sampler.sample().holds.is_empty());
    assert!(!reconciler.binding_is_held(&row.id));
}

#[tokio::test(start_paused = true)]
async fn sample_truncation_does_not_remove_hidden_protection() {
    let (_, reconciler) = setup(0, Duration::ZERO).await;
    let sampler = reconciler.binding_diagnostics();
    for index in 0..=MAX_HOLDS_PER_REPORTER {
        reconciler
            .binding_holds
            .begin(&sandbox(index), &[])
            .unwrap()
            .raced();
    }
    let sample = sampler.sample();
    assert!(sample.truncated);
    assert_eq!(sample.holds.len(), MAX_HOLDS_PER_REPORTER);
    let omitted = sandbox(MAX_HOLDS_PER_REPORTER);
    assert!(reconciler.binding_is_held(&omitted.id));
    assert!(!sample
        .holds
        .iter()
        .any(|hold| hold.sandbox_id == omitted.id));
    assert!(reconciler.binding_holds.begin(&omitted, &[]).is_none());
    tokio::time::advance(Duration::from_secs(30)).await;
    reconciler
        .binding_holds
        .begin(&sandbox(0), &[])
        .unwrap()
        .complete();
    assert!(!sampler.sample().truncated);
    assert!(sampler
        .sample()
        .holds
        .iter()
        .any(|hold| hold.sandbox_id == omitted.id));
}

#[tokio::test(start_paused = true)]
async fn cancellation_and_store_unavailability_do_not_invent_adapter_failures() {
    let (_, reconciler) = setup(0, Duration::ZERO).await;
    let sampler = reconciler.binding_diagnostics();
    let row = sandbox(0);
    drop(reconciler.binding_holds.begin(&row, &[]).unwrap());
    assert_eq!(sampler.sample().holds[0].reason, HoldReason::Cancelled);
    assert_eq!(sampler.sample().holds[0].adapter_error_class, None);
    reconciler.binding_holds.failed(&row.id, "unavailable");
    assert_eq!(sampler.sample().holds[0].reason, HoldReason::Unavailable);
    assert_eq!(sampler.sample().holds[0].adapter_error_class, None);
}

#[tokio::test(start_paused = true)]
async fn oversized_identities_are_omitted_without_releasing_protection() {
    let (_, reconciler) = setup(0, Duration::ZERO).await;
    let sampler = reconciler.binding_diagnostics();
    let mut invalid_owner = sandbox(0);
    invalid_owner.owner = layer2_core::SubjectId("o".repeat(513));
    let mut invalid_id = sandbox(1);
    invalid_id.id = layer2_core::SandboxId("s".repeat(257));
    for row in [&invalid_owner, &invalid_id] {
        reconciler.binding_holds.begin(row, &[]).unwrap().raced();
    }
    let sample = sampler.sample();
    assert!(sample.truncated);
    assert!(sample.holds.is_empty());
    for row in [&invalid_owner, &invalid_id] {
        assert!(reconciler.binding_is_held(&row.id));
    }
}
