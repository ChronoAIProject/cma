use super::fixture::GatedProvider;
use super::*;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[tokio::test(start_paused = true)]
async fn concurrent_full_ledgers_share_status_capacity_and_cancellation_releases_it() {
    let (store, _, _, registration) = setup(MAX_MEMBER_EFFECTS);
    let original = store.0.lock().unwrap().effects.clone().unwrap();
    let provider = GatedProvider::new();
    let worker = EffectReconciler::new(store.clone(), provider.clone());
    let mut attempts: Vec<_> = (0..4)
        .map(|_| Box::pin(worker.reconcile(&registration)))
        .collect();
    for _ in 0..8 {
        for attempt in &mut attempts {
            assert!(futures_util::poll!(attempt.as_mut()).is_pending());
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(provider.calls.lock().unwrap().len(), 16);
    assert_eq!(provider.active.load(Ordering::SeqCst), 16);
    drop(attempts.remove(0));
    for _ in 0..8 {
        for attempt in &mut attempts {
            assert!(futures_util::poll!(attempt.as_mut()).is_pending());
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(provider.calls.lock().unwrap().len(), 32);
    assert_eq!(provider.peak.load(Ordering::SeqCst), 16);
    drop(attempts);
    assert_eq!(provider.active.load(Ordering::SeqCst), 0);
    assert_eq!(store.0.lock().unwrap().effects.as_ref().unwrap(), &original);
    assert!(provider.calls.lock().unwrap().iter().all(|intent| original
        .iter()
        .any(|effect| &effect.permission.intent == intent)));

    provider.gate.add_permits(MAX_MEMBER_EFFECTS as usize);
    let result = worker.reconcile(&registration).await.unwrap();
    assert_eq!(result.terminal, MAX_MEMBER_EFFECTS);
    assert_eq!(result.unresolved, 0);
    assert!(result.failures.is_empty());
    let state = store.0.lock().unwrap();
    for (before, after) in original.iter().zip(state.effects.as_ref().unwrap()) {
        assert_eq!(after.permission, before.permission);
        assert_eq!(after.observations.len(), 1);
        assert_eq!(after.observations[0].outcome, Outcome::CommitConfirmed);
    }
}

#[tokio::test(start_paused = true)]
async fn status_timeout_starts_after_shared_capacity_becomes_available() {
    let (store, _, _, registration) = setup(16);
    let original = store.0.lock().unwrap().effects.clone().unwrap();
    let provider = GatedProvider::new();
    let worker = EffectReconciler::new(store.clone(), provider.clone());
    let mut first = Box::pin(worker.reconcile(&registration));
    let mut queued = Box::pin(worker.reconcile(&registration));
    assert!(futures_util::poll!(first.as_mut()).is_pending());
    assert!(futures_util::poll!(queued.as_mut()).is_pending());
    assert_eq!(provider.calls.lock().unwrap().len(), 16);
    tokio::time::advance(Duration::from_secs(2)).await;
    let expired = first.await.unwrap();
    assert_eq!((expired.terminal, expired.unresolved), (0, 16));
    assert_eq!(expired.failures.len(), 16);
    assert!(expired
        .failures
        .iter()
        .all(|(_, e)| *e == ExecutionError::Unknown));
    assert!(futures_util::poll!(queued.as_mut()).is_pending());
    assert_eq!(provider.calls.lock().unwrap().len(), 32);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(futures_util::poll!(queued.as_mut()).is_pending());
    assert_eq!(store.0.lock().unwrap().effects.as_ref().unwrap(), &original);
    provider.gate.add_permits(16);
    let recovered = queued.await.unwrap();
    assert_eq!((recovered.terminal, recovered.unresolved), (16, 0));
    assert!(recovered.failures.is_empty());
    assert_eq!(provider.active.load(Ordering::SeqCst), 0);
    assert_eq!(provider.peak.load(Ordering::SeqCst), 16);
}

#[tokio::test(start_paused = true)]
async fn retained_terminal_results_bypass_saturated_status_capacity() {
    let (store, _, _, registration) = setup(16);
    let provider = GatedProvider::new();
    let worker = EffectReconciler::new(store.clone(), provider.clone());
    let mut active = Box::pin(worker.reconcile(&registration));
    assert!(futures_util::poll!(active.as_mut()).is_pending());
    assert_eq!(provider.active.load(Ordering::SeqCst), 16);
    {
        let mut state = store.0.lock().unwrap();
        for (i, effect) in state.effects.as_mut().unwrap().iter_mut().enumerate() {
            append(
                effect,
                if i % 2 == 0 {
                    Outcome::CommitConfirmed
                } else {
                    Outcome::Rejected
                },
            );
        }
    }
    let retained = store.0.lock().unwrap().effects.clone().unwrap();
    let mut reader = Box::pin(worker.reconcile(&registration));
    let std::task::Poll::Ready(result) = futures_util::poll!(reader.as_mut()) else {
        panic!("retained terminal outcomes must remain readable without a provider slot")
    };
    assert_eq!(
        result.unwrap(),
        EffectProgress {
            terminal: 16,
            unresolved: 0,
            failures: vec![]
        }
    );
    assert_eq!(provider.calls.lock().unwrap().len(), 16);
    assert_eq!(provider.active.load(Ordering::SeqCst), 16);
    drop(active);
    assert_eq!(provider.active.load(Ordering::SeqCst), 0);
    assert_eq!(store.0.lock().unwrap().effects.as_ref().unwrap(), &retained);
}

#[tokio::test]
async fn provider_errors_release_capacity_for_later_effects_and_recovery() {
    let (store, _, _, registration) = setup(MAX_MEMBER_EFFECTS);
    let original = store.0.lock().unwrap().effects.clone().unwrap();
    let provider = GatedProvider::new();
    provider.unavailable.store(true, Ordering::SeqCst);
    let worker = EffectReconciler::new(store.clone(), provider.clone());
    let failed = tokio::time::timeout(Duration::from_secs(2), worker.reconcile(&registration))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (failed.terminal, failed.unresolved),
        (0, MAX_MEMBER_EFFECTS)
    );
    assert_eq!(failed.failures.len(), MAX_MEMBER_EFFECTS as usize);
    assert!(failed
        .failures
        .iter()
        .all(|(_, e)| *e == ExecutionError::Unavailable));
    assert_eq!(
        provider.calls.lock().unwrap().len(),
        MAX_MEMBER_EFFECTS as usize
    );
    assert_eq!(provider.active.load(Ordering::SeqCst), 0);
    assert_eq!(store.0.lock().unwrap().effects.as_ref().unwrap(), &original);

    provider.unavailable.store(false, Ordering::SeqCst);
    provider.gate.add_permits(MAX_MEMBER_EFFECTS as usize);
    let result = tokio::time::timeout(Duration::from_secs(2), worker.reconcile(&registration))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (result.terminal, result.unresolved),
        (MAX_MEMBER_EFFECTS, 0)
    );
    assert!(result.failures.is_empty());
    assert!(provider.peak.load(Ordering::SeqCst) <= 16);
    let calls = provider.calls.lock().unwrap();
    let state = store.0.lock().unwrap();
    for (before, after) in original.iter().zip(state.effects.as_ref().unwrap()) {
        assert_eq!(after.permission, before.permission);
        assert_eq!(after.observations.len(), 1);
        assert_eq!(after.observations[0].outcome, Outcome::CommitConfirmed);
        assert_eq!(
            calls
                .iter()
                .filter(|intent| *intent == &before.permission.intent)
                .count(),
            2
        );
    }
}
