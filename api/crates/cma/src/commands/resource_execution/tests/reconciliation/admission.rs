use super::fixture::GatedProvider;
use super::*;
use std::{sync::atomic::Ordering, time::Duration};

#[tokio::test(start_paused = true)]
async fn saturated_reconciliation_refuses_before_reads_and_cancelled_capture_releases_slot() {
    let (store, provider, worker, registration) = setup(MAX_MEMBER_EFFECTS);
    let original = store.0.lock().unwrap().effects.clone().unwrap();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    store.0.lock().unwrap().record_gate = Some(gate);
    let mut attempts: Vec<_> = (0..4)
        .map(|_| Box::pin(worker.reconcile(&registration)))
        .collect();
    for attempt in &mut attempts {
        assert!(futures_util::poll!(attempt.as_mut()).is_pending());
    }
    assert_eq!(store.0.lock().unwrap().record_reads, 4);
    for _ in 0..32 {
        assert_eq!(
            worker.reconcile(&registration).await,
            Err(ExecutionError::Capacity)
        );
    }
    {
        let state = store.0.lock().unwrap();
        assert_eq!(state.record_reads, 4);
        assert!(state.effect_pages.is_empty());
        assert!(state.events.is_empty());
        assert_eq!(state.effects.as_ref().unwrap(), &original);
    }
    assert!(provider.calls.lock().unwrap().is_empty());
    drop(attempts.pop());
    store.0.lock().unwrap().record_gate = None;
    *provider.reply.lock().unwrap() = Ok(None);
    let resumed = worker.reconcile(&registration).await.unwrap();
    assert_eq!(
        (resumed.terminal, resumed.unresolved),
        (0, MAX_MEMBER_EFFECTS)
    );
    assert_eq!(store.0.lock().unwrap().record_reads, 5);
    assert_eq!(store.0.lock().unwrap().effects.as_ref().unwrap(), &original);
    drop(attempts);
}

#[tokio::test(start_paused = true)]
async fn one_read_deadline_covers_record_and_later_pages_without_provider_calls() {
    for blocked_page in [false, true] {
        let (store, provider, worker, registration) = setup(33);
        let original = store.0.lock().unwrap().effects.clone().unwrap();
        let record_gate = Arc::new(tokio::sync::Semaphore::new(0));
        let page_gate = Arc::new(tokio::sync::Semaphore::new(0));
        {
            let mut state = store.0.lock().unwrap();
            state.record_gate = Some(record_gate.clone());
            state.effect_page_gate = Some((32, page_gate));
        }
        let mut attempt = Box::pin(worker.reconcile(&registration));
        assert!(futures_util::poll!(attempt.as_mut()).is_pending());
        let mut occupied: Vec<_> = (0..3)
            .map(|_| Box::pin(worker.reconcile(&registration)))
            .collect();
        for other in &mut occupied {
            assert!(futures_util::poll!(other.as_mut()).is_pending());
        }
        tokio::time::advance(Duration::from_secs(4)).await;
        if blocked_page {
            record_gate.add_permits(1);
            assert!(futures_util::poll!(attempt.as_mut()).is_pending());
            assert_eq!(store.0.lock().unwrap().effect_pages, [0, 32]);
        }
        tokio::time::advance(Duration::from_secs(1)).await;
        let std::task::Poll::Ready(result) = futures_util::poll!(attempt.as_mut()) else {
            panic!("ledger capture must retain one deadline across all pages")
        };
        assert_eq!(result, Err(ExecutionError::Unavailable));
        assert!(provider.calls.lock().unwrap().is_empty());
        {
            let mut state = store.0.lock().unwrap();
            assert_eq!(state.effects.as_ref().unwrap(), &original);
            assert!(state.events.is_empty());
            state.record_gate = None;
            state.effect_page_gate = None;
        }
        *provider.reply.lock().unwrap() = Ok(None);
        assert_eq!(
            worker.reconcile(&registration).await.unwrap().unresolved,
            33
        );
        drop(occupied);
    }
}

#[tokio::test(start_paused = true)]
async fn admission_covers_provider_waits_and_ledger_errors_do_not_leak_capacity() {
    let (store, _, _, registration) = setup(MAX_MEMBER_EFFECTS);
    let original = store.0.lock().unwrap().effects.clone().unwrap();
    let provider = GatedProvider::new();
    let worker = EffectReconciler::new(store.clone(), provider.clone());
    let mut attempts: Vec<_> = (0..4)
        .map(|_| Box::pin(worker.reconcile(&registration)))
        .collect();
    for attempt in &mut attempts {
        assert!(futures_util::poll!(attempt.as_mut()).is_pending());
    }
    let reads = store.0.lock().unwrap().effect_pages.clone();
    assert_eq!(provider.active.load(Ordering::SeqCst), 16);
    assert_eq!(
        worker.reconcile(&registration).await,
        Err(ExecutionError::Capacity)
    );
    assert_eq!(store.0.lock().unwrap().record_reads, 4);
    assert_eq!(store.0.lock().unwrap().effect_pages, reads);
    drop(attempts.remove(0));
    store.0.lock().unwrap().effects.as_mut().unwrap().pop();
    for _ in 0..8 {
        assert_eq!(
            worker.reconcile(&registration).await,
            Err(ExecutionError::Corrupt)
        );
    }
    assert_eq!(provider.calls.lock().unwrap().len(), 16);
    store.0.lock().unwrap().effects = Some(original.clone());
    drop(attempts);
    assert_eq!(provider.active.load(Ordering::SeqCst), 0);
    provider.gate.add_permits(MAX_MEMBER_EFFECTS as usize);
    assert_eq!(
        worker.reconcile(&registration).await.unwrap().terminal,
        MAX_MEMBER_EFFECTS
    );
    let state = store.0.lock().unwrap();
    for (before, after) in original.iter().zip(state.effects.as_ref().unwrap()) {
        assert_eq!(before.permission, after.permission);
        assert_eq!(after.observations.len(), 1);
    }
}
