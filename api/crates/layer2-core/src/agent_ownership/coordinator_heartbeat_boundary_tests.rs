use super::*;
use crate::agent_ownership::MaintenanceStep;

#[tokio::test(start_paused = true)]
async fn transient_retries_wait_until_the_original_deadline_even_after_committed_lost_replies() {
    for mode in [1, 6, 7] {
        let fixture = Fixture::with_policy(0, MaintenancePolicy::ContainFailures).await;
        fixture.store.heartbeat_mode.store(mode, Ordering::SeqCst);
        let started = Instant::now();
        loop {
            match fixture.manager.maintain_step().await {
                Ok(MaintenanceStep::RetryAt(next)) => {
                    assert!(fixture.manager.is_ready());
                    assert!(next >= Instant::now());
                    tokio::time::sleep_until(next).await;
                }
                Err(OwnershipError::Stale) => break,
                _ => panic!("transient outage must only retry or expire"),
            }
        }
        assert_eq!(started.elapsed(), Duration::from_secs(15));
        let calls = fixture.store.heartbeat_starts.lock().unwrap();
        assert!(calls.len() >= 2);
        assert!(calls.windows(2).all(|pair| pair[1] > pair[0]));
        let cap = 3 + (15_000_u64 - 750).div_ceil(1_000) as usize;
        assert!(calls.len() <= cap);
        assert!(!fixture.manager.is_ready());
        assert!(*fixture.manager.failure_signal().borrow());
    }
}

#[tokio::test(start_paused = true)]
async fn final_short_immediate_failure_cannot_hot_loop_or_fail_early() {
    let fixture = Fixture::with_policy(0, MaintenancePolicy::ContainFailures).await;
    fixture.store.heartbeat_mode.store(7, Ordering::SeqCst);
    let until = Instant::now() + Duration::from_millis(200);
    *fixture.manager.member_deadline.lock().unwrap() = until;
    for _ in 0..50 {
        assert!(
            matches!(fixture.manager.maintain_step().await, Ok(MaintenanceStep::RetryAt(next)) if next == until)
        );
        assert!(fixture.manager.is_ready());
    }
    assert_eq!(fixture.store.heartbeat_starts.lock().unwrap().len(), 1);
    tokio::time::advance(Duration::from_millis(199)).await;
    assert!(
        matches!(fixture.manager.maintain_step().await, Ok(MaintenanceStep::RetryAt(next)) if next == until)
    );
    tokio::time::advance(Duration::from_millis(1)).await;
    assert!(matches!(
        fixture.manager.maintain_step().await,
        Err(OwnershipError::Stale)
    ));
    assert_eq!(fixture.store.heartbeat_starts.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn recovered_reply_before_at_or_after_old_deadline_cannot_revive_membership() {
    for elapsed in [499, 500, 501] {
        let fixture = Fixture::with_policy(0, MaintenancePolicy::ContainFailures).await;
        *fixture.manager.member_deadline.lock().unwrap() =
            Instant::now() + Duration::from_millis(500);
        fixture.store.heartbeat_mode.store(12, Ordering::SeqCst);
        let call = tokio::spawn({
            let manager = fixture.manager.clone();
            async move { manager.maintain_step().await }
        });
        fixture.store.entered.notified().await;
        tokio::time::advance(Duration::from_millis(elapsed)).await;
        fixture.store.resume.notify_one();
        if elapsed < 500 {
            assert!(matches!(
                call.await.unwrap(),
                Ok(MaintenanceStep::Confirmed { .. })
            ));
            assert!(fixture.manager.is_ready());
        } else {
            assert!(matches!(call.await.unwrap(), Err(OwnershipError::Stale)));
            assert!(!fixture.manager.is_ready());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn adoption_stays_closed_until_every_owner_is_normalized() {
    let fixture = Fixture::with_policy(2, MaintenancePolicy::ContainFailures).await;
    fixture.store.gate.store(true, Ordering::SeqCst);
    *fixture.store.reply.lock().unwrap() = Reply::Renew;
    let call = tokio::spawn({
        let manager = fixture.manager.clone();
        async move { manager.maintain_step().await }
    });
    fixture.store.entered.notified().await;
    assert!(fixture
        .manager
        .acquire(&crate::AgentId("new".into()))
        .await
        .unwrap()
        .is_none());
    assert!(Arc::ptr_eq(
        &fixture
            .manager
            .acquire(&fixture.owners[1].token.agent_id)
            .await
            .unwrap()
            .unwrap(),
        &fixture.owners[1]
    ));
    fixture.store.resume.notify_one();
    assert!(matches!(
        call.await.unwrap(),
        Ok(MaintenanceStep::Confirmed { .. })
    ));
    assert!(fixture
        .manager
        .acquire(&crate::AgentId("new".into()))
        .await
        .unwrap()
        .is_some());
}

#[tokio::test(start_paused = true)]
async fn owner_expiring_between_membership_reply_and_dispatch_keeps_exact_provenance() {
    let fixture = Fixture::with_policy(9, MaintenancePolicy::ContainFailures).await;
    fixture.owners[8]
        .renew_until(Instant::now() + Duration::from_millis(1_200))
        .unwrap();
    fixture.store.heartbeat_mode.store(7, Ordering::SeqCst);
    let Ok(MaintenanceStep::RetryAt(next)) = fixture.manager.maintain_step().await else {
        panic!("expected retry")
    };
    tokio::time::sleep_until(next).await;
    fixture.store.heartbeat_mode.store(0, Ordering::SeqCst);
    fixture.store.all.store(true, Ordering::SeqCst);
    *fixture.store.reply.lock().unwrap() = Reply::Renew;
    *fixture.store.delay.lock().unwrap() = Duration::from_millis(900);
    assert!(matches!(
        fixture.manager.maintain_step().await,
        Ok(MaintenanceStep::Confirmed { .. })
    ));
    assert!(fixture.owners[8].retirement_pending());
    assert!(!fixture.owners[8].receiver_cancelled());
    assert!(fixture.owners[..8]
        .iter()
        .all(|authority| authority.is_current()));
    assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 8);
    assert!(fixture.manager.is_ready());
}

#[tokio::test(start_paused = true)]
async fn unmarked_expiry_and_generic_revocation_remain_fatal_during_retry() {
    for generic in [false, true] {
        let fixture = Fixture::with_policy(1, MaintenancePolicy::ContainFailures).await;
        if generic {
            fixture.owners[0].revoke();
        } else {
            fixture.owners[0]
                .renew_until(Instant::now() + Duration::from_millis(1))
                .unwrap();
            tokio::time::advance(Duration::from_millis(1)).await;
        }
        fixture.store.heartbeat_mode.store(7, Ordering::SeqCst);
        let Ok(MaintenanceStep::RetryAt(next)) = fixture.manager.maintain_step().await else {
            panic!("expected retry")
        };
        tokio::time::sleep_until(next).await;
        fixture.store.heartbeat_mode.store(0, Ordering::SeqCst);
        assert!(matches!(
            fixture.manager.maintain_step().await,
            Err(OwnershipError::Stale)
        ));
        assert!(!fixture.owners[0].retirement_pending());
        assert!(!fixture.owners[0].receiver_cancelled());
    }
}

#[tokio::test(start_paused = true)]
async fn drain_interrupts_heartbeat_call_and_backoff_without_publishing_failure() {
    for during_call in [false, true] {
        let fixture = Fixture::with_policy(0, MaintenancePolicy::ContainFailures).await;
        if during_call {
            fixture.store.heartbeat_mode.store(11, Ordering::SeqCst);
            let call = tokio::spawn({
                let manager = fixture.manager.clone();
                async move { manager.maintain_step().await }
            });
            fixture.store.entered.notified().await;
            fixture.manager.stop_admission();
            assert!(matches!(call.await.unwrap(), Ok(MaintenanceStep::Draining)));
        } else {
            fixture.store.heartbeat_mode.store(7, Ordering::SeqCst);
            assert!(matches!(
                fixture.manager.maintain_step().await,
                Ok(MaintenanceStep::RetryAt(_))
            ));
            fixture.manager.stop_admission();
            assert!(matches!(
                fixture.manager.maintain_step().await,
                Ok(MaintenanceStep::Draining)
            ));
        }
        assert!(!*fixture.manager.failure_signal().borrow());
        assert_eq!(fixture.store.heartbeat_starts.lock().unwrap().len(), 1);
    }
}
