use super::super::authority::ReceiverCancellation;
use super::*;

#[path = "coordinator_late_renewal_tests.rs"]
mod late_reply;
#[path = "coordinator_renewal_test_support.rs"]
mod support;
use support::{Fixture, Reply};
#[path = "coordinator_batch_renewal_tests.rs"]
mod batch;
#[path = "coordinator_heartbeat_containment_tests.rs"]
mod containment;
#[path = "coordinator_renewal_diagnostic_tests.rs"]
mod diagnostics;
#[path = "coordinator_heartbeat_tests.rs"]
mod heartbeat;

#[tokio::test(start_paused = true)]
async fn unavailable_renewal_cancels_only_live_authority_and_does_not_replay() {
    let fixture = Fixture::new(2).await;
    let original = fixture
        .store
        .owner(&fixture.owners[0].token.agent_id)
        .await
        .unwrap();
    let result = fixture.manager.maintain().await;
    assert!(
        fixture.owners[1].is_current(),
        "isolated renewal cancelled the healthy peer: {:?}",
        result.as_ref().map(Vec::len)
    );
    result.unwrap();
    assert!(fixture.manager.is_ready());
    assert!(!*fixture.manager.failure_signal().borrow());
    assert!(!fixture.owners[0].is_current());
    assert!(fixture.owners[0].receiver_cancelled());
    assert!(Arc::ptr_eq(
        &fixture.manager.cancelled_receivers().await[0],
        &fixture.owners[0]
    ));
    assert_eq!(
        fixture
            .store
            .owner(&fixture.owners[0].token.agent_id)
            .await
            .unwrap(),
        original
    );
    fixture.manager.maintain().await.unwrap();
    assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.owners[0]
            .run(async { panic!("cancelled effect ran") })
            .await,
        Err(OwnershipError::Stale)
    );
    fixture.owners[1]
        .run(fixture.store.check(&fixture.owners[1].token))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn operation_timeout_contains_before_authority_deadline_but_not_at_or_after_it() {
    for deadline_ms in [2_001, 2_000, 1_999] {
        let fixture = Fixture::new(2).await;
        *fixture.store.delay.lock().unwrap() = Duration::from_secs(10);
        fixture.owners[0]
            .renew_until(Instant::now() + Duration::from_millis(deadline_ms))
            .unwrap();
        let result = fixture.manager.maintain().await;
        if deadline_ms > STORE_DEADLINE_MS {
            assert!(result.is_ok());
            assert!(fixture.manager.is_ready());
            assert!(fixture.owners[0].receiver_cancelled());
            assert!(fixture.owners[1].is_current());
        } else {
            assert!(matches!(result, Err(OwnershipError::Stale)));
            fixture.assert_failed();
            assert!(
                !fixture.owners[0].receiver_cancelled(),
                "expiry was relabelled as containment"
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn invalid_membership_and_non_transient_heartbeat_errors_remain_process_fatal() {
    // Protocol, drain, missing member, Stale, IncompatibleProtocol and Invalid.
    for (policy, mode) in [
        MaintenancePolicy::Legacy,
        MaintenancePolicy::ContainFailures,
    ]
    .into_iter()
    .flat_map(|policy| [2, 3, 4, 8, 9, 10, 13, 14, 16].map(|mode| (policy, mode)))
    {
        let fixture = Fixture::with_policy(2, policy).await;
        fixture.store.heartbeat_mode.store(mode, Ordering::SeqCst);
        assert!(fixture.manager.maintain().await.is_err());
        fixture.assert_failed();
        assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 0);
        assert!(!fixture.owners[0].receiver_cancelled());
    }
}

#[tokio::test(start_paused = true)]
async fn membership_expiry_during_a_call_and_a_four_wide_batch_is_fatal() {
    for count in [2, 64] {
        let fixture = Fixture::new(count).await;
        *fixture.store.delay.lock().unwrap() = Duration::from_secs(10);
        fixture.store.all.store(true, Ordering::SeqCst);
        if count == 2 {
            fixture.store.heartbeat_mode.store(5, Ordering::SeqCst);
        }
        let started = Instant::now();
        assert!(matches!(
            fixture.manager.maintain().await,
            Err(OwnershipError::Stale)
        ));
        fixture.assert_failed();
        assert_eq!(
            started.elapsed(),
            if count == 2 {
                Duration::from_millis(500)
            } else {
                Duration::from_secs(15)
            }
        );
        assert!(fixture.store.maximum.load(Ordering::SeqCst) <= 4);
        assert!(fixture.store.calls.load(Ordering::SeqCst) <= count);
        if count == 64 {
            assert!(fixture.store.calls.load(Ordering::SeqCst) < count);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn active_stale_protocol_invalid_token_and_deadline_errors_are_fatal() {
    for policy in [
        MaintenancePolicy::Legacy,
        MaintenancePolicy::ContainFailures,
    ] {
        for reply in [
            Reply::Error(OwnershipError::Stale),
            Reply::Error(OwnershipError::IncompatibleProtocol),
            Reply::Error(OwnershipError::Invalid("synthetic invalid detail".into())),
            Reply::DifferentToken("agent"),
            Reply::DifferentToken("instance"),
            Reply::DifferentToken("epoch"),
            Reply::DifferentToken("protocol"),
            Reply::ExpiredDeadline,
        ] {
            let fixture = Fixture::with_policy(2, policy).await;
            *fixture.store.reply.lock().unwrap() = reply;
            assert!(fixture.manager.maintain().await.is_err());
            fixture.assert_failed();
            assert!(!fixture.owners[0].receiver_cancelled());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn generic_revocation_and_deadline_equality_cannot_be_relabelled_as_containment() {
    for expired in [false, true] {
        let fixture = Fixture::new(1).await;
        if expired {
            tokio::time::advance(Duration::from_secs(15)).await;
        } else {
            fixture.owners[0].revoke();
        }
        assert_eq!(
            fixture.owners[0].cancel_receiver(),
            ReceiverCancellation::NotLive
        );
        assert!(!fixture.owners[0].receiver_cancelled());
        assert!(fixture.manager.maintain().await.is_err());
        fixture.assert_failed();
    }
    let fixture = Fixture::new(1).await;
    assert_eq!(
        fixture.owners[0].cancel_receiver(),
        ReceiverCancellation::CancelledWhileLive
    );
    tokio::time::advance(Duration::from_secs(15)).await;
    assert_eq!(
        fixture.owners[0].cancel_receiver(),
        ReceiverCancellation::AlreadySafelyCancelled
    );
    assert!(!fixture.owners[0].is_current());
}

#[tokio::test(start_paused = true)]
async fn concurrent_process_failure_wins_over_unavailable_renewal() {
    let fixture = Fixture::new(2).await;
    fixture.store.gate.store(true, Ordering::SeqCst);
    let running = tokio::spawn({
        let manager = fixture.manager.clone();
        async move { manager.maintain().await }
    });
    fixture.store.entered.notified().await;
    fixture.manager.fail().await;
    fixture.store.resume.notify_one();
    assert!(running.await.unwrap().is_err());
    fixture.assert_failed();
    assert!(!fixture.owners[0].receiver_cancelled());
}

#[tokio::test(start_paused = true)]
async fn prior_safe_cancellation_does_not_hide_protocol_or_invalid_record_failures() {
    for error in [
        OwnershipError::Unavailable("synthetic unavailable".into()),
        OwnershipError::Stale,
        OwnershipError::IncompatibleProtocol,
        OwnershipError::Invalid("synthetic invalid".into()),
    ] {
        let fatal = matches!(
            error,
            OwnershipError::IncompatibleProtocol | OwnershipError::Invalid(_)
        );
        let fixture = Fixture::new(2).await;
        *fixture.store.reply.lock().unwrap() = Reply::Error(error);
        fixture.store.gate.store(true, Ordering::SeqCst);
        let running = tokio::spawn({
            let manager = fixture.manager.clone();
            async move { manager.maintain().await }
        });
        fixture.store.entered.notified().await;
        assert_eq!(
            fixture.owners[0].cancel_receiver(),
            ReceiverCancellation::CancelledWhileLive
        );
        fixture.store.resume.notify_one();
        assert_eq!(running.await.unwrap().is_err(), fatal);
        if fatal {
            fixture.assert_failed();
        } else {
            assert!(fixture.manager.is_ready());
            assert!(fixture.owners[1].is_current());
        }
        assert!(!fixture.owners[0].is_current());
    }
}

#[tokio::test(start_paused = true)]
async fn cancellation_at_the_deadline_cannot_hide_expiry_but_prior_cancellation_stays_safe() {
    for cancel_ms in [999, 1_000] {
        let fixture = Fixture::new(2).await;
        fixture.owners[0]
            .renew_until(Instant::now() + Duration::from_millis(1_000))
            .unwrap();
        fixture.store.gate.store(true, Ordering::SeqCst);
        let running = tokio::spawn({
            let manager = fixture.manager.clone();
            async move { manager.maintain().await }
        });
        fixture.store.entered.notified().await;
        tokio::time::advance(Duration::from_millis(cancel_ms)).await;
        let outcome = fixture.owners[0].cancel_receiver();
        tokio::time::advance(Duration::from_millis(2)).await;
        fixture.store.resume.notify_one();
        if cancel_ms < 1_000 {
            assert_eq!(outcome, ReceiverCancellation::CancelledWhileLive);
            assert!(running.await.unwrap().is_ok());
            assert!(fixture.manager.is_ready());
            assert!(fixture.owners[1].is_current());
        } else {
            assert_eq!(outcome, ReceiverCancellation::NotLive);
            assert!(running.await.unwrap().is_err());
            fixture.assert_failed();
            assert!(!fixture.owners[0].receiver_cancelled());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn concurrent_drain_never_returns_to_serving_after_a_renewal_failure() {
    let fixture = Fixture::new(2).await;
    fixture.store.gate.store(true, Ordering::SeqCst);
    let running = tokio::spawn({
        let manager = fixture.manager.clone();
        async move { manager.maintain().await }
    });
    fixture.store.entered.notified().await;
    fixture.manager.stop_admission();
    fixture.store.resume.notify_one();
    assert!(running.await.unwrap().is_err());
    fixture.assert_failed();
    assert!(fixture.manager.is_draining());
}

#[tokio::test(start_paused = true)]
async fn renewal_reply_cannot_bring_back_retired_or_expired_authority() {
    let fixture = Fixture::new(2).await;
    let leases = fixture.leases().await;
    let retired = &fixture.owners[0];
    retired
        .begin_retirement(super::super::RetirementIntent::Cooperative)
        .unwrap();
    let started = Instant::now();
    assert_eq!(
        fixture
            .manager
            .apply_renewal(retired, started, 0, &leases[0]),
        Ok(())
    );
    assert!(!retired.is_current());
    assert!(retired.retirement_pending());
    assert!(matches!(
        fixture.manager.apply_renewal(
            &fixture.owners[1],
            started - Duration::from_secs(30),
            0,
            &leases[1]
        ),
        Err(OwnershipError::Stale)
    ));
    assert!(fixture.manager.is_ready());
}

#[tokio::test(start_paused = true)]
async fn unavailable_renewal_preserves_retiring_custody_and_refuses_after_membership_loss() {
    let fixture = Fixture::new(2).await;
    let retired = &fixture.owners[0];
    retired
        .begin_retirement(super::super::RetirementIntent::Cooperative)
        .unwrap();
    assert_eq!(
        fixture
            .manager
            .contain_unavailable_renewal(retired, "fixture"),
        Ok(())
    );
    assert!(retired.retirement_pending());
    let still_live = &fixture.owners[1];
    *fixture.manager.member_deadline.lock().unwrap() = Instant::now();
    assert!(matches!(
        fixture
            .manager
            .contain_unavailable_renewal(still_live, "fixture"),
        Err(OwnershipError::Stale)
    ));
    assert!(!still_live.receiver_cancelled());
    assert_eq!(fixture.manager.owned().await.len(), 2);
}
