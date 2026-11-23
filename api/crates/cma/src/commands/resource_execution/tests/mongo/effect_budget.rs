use super::super::super::reconciliation::EffectReconciler;
use super::super::reconciliation::fixture::{GatedProvider, Provider};
use super::*;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn excess_reconciliation_keeps_original_ledger_and_reuses_capacity_after_cancellation() {
    let fixture = Fixture::new().await;
    let effect = effects::prepare(&fixture).await;
    let registration = &effect.permission.intent.registration;
    let provider = GatedProvider::new();
    let worker = EffectReconciler::new(fixture.store.clone(), provider.clone());
    let mut attempts: Vec<_> = (0..4)
        .map(|_| Box::pin(worker.reconcile(registration)))
        .collect();
    for attempt in &mut attempts {
        assert!(futures_util::poll!(attempt.as_mut()).is_pending());
    }
    for _ in 0..32 {
        assert_eq!(
            worker.reconcile(registration).await,
            Err(ExecutionError::Capacity)
        );
    }
    assert_eq!(
        fixture
            .store
            .effects_for(registration, 0, 32)
            .await
            .unwrap(),
        vec![effect.clone()]
    );
    let operation = fixture.freeze().await;
    let final_receipt = effects::complete(&fixture, &effect, &operation).await;
    assert_eq!(
        fixture
            .store
            .effects_for(registration, 0, 32)
            .await
            .unwrap(),
        vec![effect.clone()]
    );
    drop(attempts);
    assert_eq!(provider.active.load(Ordering::SeqCst), 0);
    provider.gate.add_permits(1);
    assert_eq!(worker.reconcile(registration).await.unwrap().terminal, 1);

    let reopened = fixture.reopened().await;
    let provider = Provider::new(&effect, wire::EffectOutcome::CommitConfirmed);
    let recovered = EffectReconciler::new(reopened.clone(), provider.clone());
    assert_eq!(recovered.reconcile(registration).await.unwrap().terminal, 1);
    assert!(provider.calls.lock().unwrap().is_empty());
    let records = reopened.effects_for(registration, 0, 32).await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].permission, effect.permission);
    assert_eq!(records[0].observations.len(), 2);
    assert_eq!(records[0].observations[0], effect.observations[0]);
    assert_eq!(
        records[0].observations[1].outcome,
        wire::EffectOutcome::CommitConfirmed
    );
    assert!(
        reopened
            .revocation(&operation.key.operation_id)
            .await
            .unwrap()
            .unwrap()
            == final_receipt
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn cancelled_status_polls_preserve_effect_and_revocation_receipt_across_reopen() {
    let fixture = Fixture::new().await;
    let effect = effects::prepare(&fixture).await;
    let registration = &effect.permission.intent.registration;
    let provider = GatedProvider::new();
    let worker = Arc::new(EffectReconciler::new(
        fixture.store.clone(),
        provider.clone(),
    ));
    let mut attempts = tokio::task::JoinSet::new();
    for _ in 0..32 {
        let worker = worker.clone();
        let registration = registration.clone();
        attempts.spawn(async move { worker.reconcile(&registration).await });
    }
    tokio::time::timeout(Duration::from_secs(10), provider.wait_for_call())
        .await
        .unwrap();
    // Status-only polling has no claim on participant stop/finalization capacity.
    let operation = fixture.freeze().await;
    let final_receipt = tokio::time::timeout(
        Duration::from_secs(10),
        effects::complete(&fixture, &effect, &operation),
    )
    .await
    .unwrap();
    attempts.abort_all();
    while let Some(result) = attempts.join_next().await {
        match result {
            Err(error) => assert!(error.is_cancelled()),
            Ok(Err(ExecutionError::Capacity)) => {}
            Ok(result) => {
                let result = result.unwrap();
                assert_eq!((result.terminal, result.unresolved), (0, 1));
                assert_eq!(
                    result.failures,
                    vec![(
                        effect.permission.intent.effect_id.clone(),
                        ExecutionError::Unknown
                    )]
                );
            }
        }
    }
    assert_eq!(provider.active.load(Ordering::SeqCst), 0);
    assert!(provider.peak.load(Ordering::SeqCst) <= 16);
    assert!(provider
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|intent| intent == &effect.permission.intent));
    assert_eq!(
        fixture
            .store
            .effects_for(registration, 0, 32)
            .await
            .unwrap(),
        vec![effect.clone()]
    );
    assert!(final_operation(&fixture, &operation).await == final_receipt);

    let reopened = fixture.reopened().await;
    let provider = Provider::new(&effect, wire::EffectOutcome::CommitConfirmed);
    let recovered = EffectReconciler::new(reopened.clone(), provider.clone());
    assert_eq!(recovered.reconcile(registration).await.unwrap().terminal, 1);
    assert_eq!(recovered.reconcile(registration).await.unwrap().terminal, 1);
    assert_eq!(
        *provider.calls.lock().unwrap(),
        vec![effect.permission.intent.clone()]
    );
    let records = reopened.effects_for(registration, 0, 32).await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].permission, effect.permission);
    assert_eq!(records[0].observations.len(), 2);
    assert_eq!(records[0].observations[0], effect.observations[0]);
    assert_eq!(
        records[0].observations[1].outcome,
        wire::EffectOutcome::CommitConfirmed
    );
    assert!(
        reopened
            .revocation(&operation.key.operation_id)
            .await
            .unwrap()
            .unwrap()
            == final_receipt
    );
    fixture.cleanup().await;
}
