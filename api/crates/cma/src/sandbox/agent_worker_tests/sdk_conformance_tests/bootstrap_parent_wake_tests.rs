use super::*;

#[tokio::test(start_paused = true)]
async fn bootstrap_parent_readiness_wakes_current_lease_without_poll_delay() {
    for notified in [false, true] {
        let readiness = Arc::new(tokio::sync::Notify::new());
        let fixture = BootstrapHarness::with_readiness(Some(readiness.clone())).await;
        let h = &fixture.harness;
        let agent = fixture.admit(None).await;
        let before = h.backend.requests().await.len();
        let started = tokio::time::Instant::now();
        tokio::join!(h.run(&agent.id, h.now), async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            fixture.ready().await;
            if notified {
                readiness.notify_waiters();
            }
        });
        assert_eq!(
            started.elapsed(),
            Duration::from_millis(if notified { 100 } else { 250 })
        );
        assert_eq!(
            h.stored(&agent.id).await.work.step,
            AgentWorkStep::AddWorktree
        );
        assert_eq!(h.backend.requests().await.len(), before);
    }
}

#[tokio::test(start_paused = true)]
async fn bootstrap_parent_hint_does_not_grant_readiness_or_reset_the_wait_budget() {
    let readiness = Arc::new(tokio::sync::Notify::new());
    let fixture = BootstrapHarness::with_readiness(Some(readiness.clone())).await;
    let h = &fixture.harness;
    let agent = fixture.admit(None).await;
    let before = h.backend.requests().await.len();
    let started = tokio::time::Instant::now();
    tokio::join!(h.run(&agent.id, h.now), async {
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            readiness.notify_waiters();
        }
    });
    assert_eq!(started.elapsed(), Duration::from_secs(1));
    let waiting = h.stored(&agent.id).await;
    assert_eq!(waiting.work.step, AgentWorkStep::WaitForWorkspace);
    assert!(waiting.has_no_runtime_effects());
    assert!(waiting.work.lease.is_none());
    assert_eq!(h.backend.requests().await.len(), before);
}

#[tokio::test(start_paused = true)]
async fn bootstrap_parent_hint_cannot_override_stop_during_wait() {
    let readiness = Arc::new(tokio::sync::Notify::new());
    let fixture = BootstrapHarness::with_readiness(Some(readiness.clone())).await;
    let h = &fixture.harness;
    let agent = fixture.admit(None).await;
    let before = h.backend.requests().await.len();
    let started = tokio::time::Instant::now();
    tokio::join!(h.run(&agent.id, h.now), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let current = h.stored(&agent.id).await;
        h.store
            .schedule_agent_stop(AgentStopRequest {
                target: AgentControlTarget::from(&current),
                reason: AgentStopReason::Owner,
                requested_at: rfc3339_at(h.now),
            })
            .await
            .unwrap();
        fixture.ready().await;
        readiness.notify_waiters();
    });
    assert_eq!(started.elapsed(), Duration::from_millis(100));
    assert_ne!(
        h.stored(&agent.id).await.work.step,
        AgentWorkStep::AddWorktree
    );
    assert_eq!(h.backend.requests().await.len(), before);
}

#[tokio::test(start_paused = true)]
async fn bootstrap_parent_readiness_before_registration_uses_durable_state() {
    let readiness = Arc::new(tokio::sync::Notify::new());
    let fixture = BootstrapHarness::with_readiness(Some(readiness.clone())).await;
    let h = &fixture.harness;
    let agent = fixture.admit(None).await;
    fixture.ready().await;
    readiness.notify_waiters();
    let started = tokio::time::Instant::now();
    h.run(&agent.id, h.now).await;
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert_eq!(
        h.stored(&agent.id).await.work.step,
        AgentWorkStep::AddWorktree
    );
}
