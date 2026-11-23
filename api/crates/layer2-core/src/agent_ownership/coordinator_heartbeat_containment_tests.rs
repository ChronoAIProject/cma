use super::*;
#[path = "coordinator_heartbeat_boundary_tests.rs"]
mod boundaries;

#[tokio::test(start_paused = true)]
async fn transient_reply_loss_recovers_same_member_without_extending_unconfirmed_owners() {
    for mode in [1, 6, 7] {
        let fixture = Fixture::with_policy(2, MaintenancePolicy::ContainFailures).await;
        *fixture.store.reply.lock().unwrap() = Reply::Renew;
        fixture.store.all.store(true, Ordering::SeqCst);
        let original = fixture.leases().await;
        fixture.store.heartbeat_mode.store(mode, Ordering::SeqCst);
        let Ok(crate::agent_ownership::MaintenanceStep::RetryAt(next)) =
            fixture.manager.maintain_step().await
        else {
            panic!("expected bounded retry");
        };
        assert_eq!(fixture.leases().await, original);
        assert!(fixture.manager.is_ready());
        assert!(fixture
            .manager
            .acquire(&AgentId("new".into()))
            .await
            .unwrap()
            .is_none());
        tokio::time::sleep_until(next).await;
        fixture.store.heartbeat_mode.store(0, Ordering::SeqCst);
        assert!(matches!(
            fixture.manager.maintain_step().await,
            Ok(crate::agent_ownership::MaintenanceStep::Confirmed { .. })
        ));
        assert!(fixture.manager.is_ready());
        for (old, current) in original.iter().zip(fixture.leases().await) {
            assert_eq!(old.token, current.token);
            assert!(current.expires_at_ms > old.expires_at_ms);
        }
        assert!(fixture.owners.iter().all(|owner| owner.is_current()));
    }
}

#[tokio::test(start_paused = true)]
async fn duplicate_process_members_are_not_validated_as_membership_recovery() {
    let fixture = Fixture::with_policy(2, MaintenancePolicy::ContainFailures).await;
    fixture.store.heartbeat_mode.store(15, Ordering::SeqCst);
    assert!(fixture.manager.maintain_step().await.is_err());
    fixture.assert_failed();
    assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn owner_expiry_during_first_transient_heartbeat_recovers_without_process_failure() {
    let fixture = Fixture::with_policy(2, MaintenancePolicy::ContainFailures).await;
    fixture.owners[0]
        .renew_until(Instant::now() + Duration::from_secs(1))
        .unwrap();
    fixture.store.heartbeat_mode.store(1, Ordering::SeqCst);
    fixture.manager.maintain().await.unwrap();
    assert!(!fixture.owners[0].is_current());
    assert!(!fixture.owners[0].receiver_cancelled());
    assert!(fixture.manager.is_ready());
    tokio::time::sleep(Duration::from_millis(250)).await;
    fixture.store.heartbeat_mode.store(0, Ordering::SeqCst);
    *fixture.store.reply.lock().unwrap() = Reply::Renew;
    assert!(
        fixture.manager.maintain().await.is_ok(),
        "a proven retry episode must retire its expired exact owner instead of failing the process"
    );
    assert!(fixture.manager.is_ready());
    assert!(!*fixture.manager.failure_signal().borrow());
    assert!(fixture.owners[0].retirement_pending());
    assert!(fixture.owners[1].is_current());
    assert_eq!(
        fixture.store.calls.load(Ordering::SeqCst),
        0,
        "expired token must never be renewed"
    );
    fixture
        .manager
        .retirement_background(&fixture.owners[0])
        .await
        .unwrap();
    let next = fixture
        .manager
        .acquire(&fixture.owners[0].token.agent_id)
        .await
        .unwrap()
        .unwrap();
    assert!(next.token.epoch > fixture.owners[0].token.epoch);
    assert!(next.is_current());
}
