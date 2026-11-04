use super::{delivery_tests::stored, expiry::EXPIRED_BEFORE_START, test_support::*, *};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn expired_pending_is_durable_and_replayed_without_preparing_or_starting() {
    let setup = Setup::new(supported()).await;
    setup
        .server
        .memory
        .clock
        .store(fixture::NOW + 900, Ordering::SeqCst);
    setup.current.revoke_at.store(0, Ordering::SeqCst);
    assert!(matches!(setup.dispatch().await.unwrap(),
        DispatchOutcome::Terminal(Phase::Refused { code }) if code == EXPIRED_BEFORE_START));
    let original = stored(&setup).await;
    assert!(original.body.is_none());
    assert!(original.delivery.is_none());
    assert!(original.projection.is_none());
    assert_eq!(
        original.event_expires_at,
        setup.target().launch.event.expires_at
    );
    assert_eq!(original.retained_until, setup.plan.retained_until);
    let restarted = Dispatcher::new(
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
        setup.port.clone(),
    );
    for _ in 0..2 {
        assert!(
            matches!(restarted.dispatch(&setup.plan.identity, &setup.target().id, "restarted").await.unwrap(),
            DispatchOutcome::Terminal(Phase::Refused { code }) if code == EXPIRED_BEFORE_START)
        );
    }
    assert_eq!(stored(&setup).await.phase, original.phase);
    assert_eq!(setup.current.calls.load(Ordering::SeqCst), 0);
    assert_eq!(setup.port.prepares.load(Ordering::SeqCst), 0);
    assert!(setup.port.starts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn expiry_preserves_frozen_and_ambiguous_provider_starts() {
    for phase in [Phase::Prepared, Phase::Starting, Phase::Unknown] {
        let setup = Setup::new(supported()).await;
        let mut lease = setup.prepared().await;
        setup
            .journal()
            .transition(&mut lease, phase.clone())
            .await
            .unwrap();
        let original_body = lease.state.body.clone();
        setup
            .server
            .memory
            .clock
            .store(fixture::NOW + 900, Ordering::SeqCst);
        assert!(matches!(
            setup.dispatch().await,
            Err(ExecutionError::Expired)
        ));
        let retained = stored(&setup).await;
        assert_eq!(retained.phase, phase);
        assert!(retained.body == original_body);
        assert!(setup.port.starts.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn expiry_does_not_reclassify_a_provider_request_with_unknown_acceptance() {
    let setup = Setup::new(supported()).await;
    *setup.port.next.lock().unwrap() = Some(StartResult::Unknown);
    assert!(matches!(
        setup.dispatch().await.unwrap(),
        DispatchOutcome::Unknown
    ));
    setup
        .server
        .memory
        .clock
        .store(fixture::NOW + 900, Ordering::SeqCst);
    assert!(matches!(
        setup.dispatch().await,
        Err(ExecutionError::Expired)
    ));
    assert_eq!(stored(&setup).await.phase, Phase::Unknown);
    assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn unrelated_expiry_and_stale_ownership_cannot_terminalize_pending() {
    let setup = Setup::new(supported()).await;
    let mut lease = setup
        .journal()
        .claim(&setup.plan, setup.target(), "old")
        .await
        .unwrap();
    // An expired ancestor or malformed reference is not this event's expiry.
    assert_eq!(
        setup.journal().expire_pending(&mut lease).await.unwrap(),
        None
    );
    setup
        .server
        .memory
        .clock
        .store(fixture::NOW + 900, Ordering::SeqCst);
    let newer = setup
        .journal()
        .claim(&setup.plan, setup.target(), "new")
        .await
        .unwrap();
    assert_eq!(
        setup.journal().expire_pending(&mut lease).await,
        Err(ExecutionError::LostLease)
    );
    let retained = stored(&setup).await;
    assert_eq!(retained.phase, Phase::Pending);
    assert_eq!(retained.ownership, newer.state.ownership);
}
