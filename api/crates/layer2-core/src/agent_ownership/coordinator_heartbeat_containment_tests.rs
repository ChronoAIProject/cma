use super::*;
#[path = "coordinator_heartbeat_boundary_tests.rs"]
mod boundaries;

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
