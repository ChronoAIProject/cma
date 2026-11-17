use super::*;
use layer3_adapter::agent_sandbox::NativePoolControl;

async fn retire_and_prove(
    fixture: &Fixture,
    before: &Sandbox,
    agent: &layer2_core::sandbox::Agent,
    mut now: SystemTime,
) {
    let owner = caller("owner", [Capability::DeleteOwnSandbox]);
    assert!(fixture
        .harness
        .service
        .delete(&caller("other", [Capability::DeleteOwnSandbox]), &before.id)
        .await
        .is_err());
    let retiring = fixture
        .harness
        .service
        .delete(&owner, &before.id)
        .await
        .unwrap();
    assert_eq!(retiring.status, SandboxStatus::Retiring);
    assert!(retiring.work.lease.is_none());
    assert!(retiring.lifecycle.operation.is_none());
    assert_eq!(
        fixture
            .harness
            .service
            .delete(&owner, &before.id)
            .await
            .unwrap(),
        retiring
    );
    fixture.backend.hold_delete.store(true, Ordering::SeqCst);
    for _ in 0..8 {
        now += Duration::from_secs(6);
        fixture.step(&before.id, now, &runtime_image()).await;
        assert!(
            !fixture
                .harness
                .stored(&before.id)
                .await
                .lifecycle
                .home
                .as_ref()
                .unwrap()
                .released
        );
    }
    fixture.backend.hold_delete.store(false, Ordering::SeqCst);
    fixture.backend.lose_delete.store(true, Ordering::SeqCst);
    let retired = fixture
        .drive(
            &before.id,
            &mut now,
            SandboxStatus::Retired,
            &runtime_image(),
        )
        .await;
    assert!(retired.runtime.is_none() && retired.lifecycle.suspended_runtime.is_none());
    assert!(retired.work.lease.is_none());
    assert!(retired.lifecycle.home.as_ref().unwrap().released);
    assert!(
        retired
            .lifecycle
            .home
            .as_ref()
            .unwrap()
            .native_pool
            .as_ref()
            .unwrap()
            .cleaned
    );
    assert!(fixture
        .backend
        .pools
        .as_ref()
        .unwrap()
        .inventory_claims()
        .await
        .unwrap()
        .is_empty());
    assert!(fixture
        .adapter(&runtime_image())
        .runtime_inventory()
        .await
        .unwrap()
        .is_empty());
    let stopped = fixture
        .harness
        .store
        .get_agent(&agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert!(stopped.work.lease.is_none());
    assert_eq!(
        stopped.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert_eq!(
        fixture
            .harness
            .service
            .delete(&owner, &before.id)
            .await
            .unwrap(),
        retired
    );
    assert_eq!(
        fixture
            .backend
            .calls
            .snapshot()
            .iter()
            .filter(|c| c.as_str() == "delete-effective")
            .count(),
        1
    );
}

#[tokio::test]
async fn conversation_reopen_rejection_then_owner_delete_converges_after_lost_reply() {
    let (fixture, reopening, agent, mut now) = retained_reopen_tests::restored_reopen().await;
    fixture.backend.reject_reopen.store(true, Ordering::SeqCst);
    now += Duration::from_secs(6);
    fixture.step(&reopening.id, now, &runtime_image()).await;
    let failed = fixture.harness.stored(&reopening.id).await;
    assert_eq!(failed.status, SandboxStatus::Failed);
    assert_eq!(failed.failure.as_ref().unwrap().class, "restore_failed");
    assert_eq!(failed.lifecycle.home, reopening.lifecycle.home);
    assert_eq!(
        fixture
            .harness
            .store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Paused
    );
    retire_and_prove(&fixture, &failed, &agent, now).await;
}

#[tokio::test]
async fn owner_delete_fences_a_silent_inflight_reopen_and_competing_worker() {
    let (fixture, reopening, agent, mut now) = retained_reopen_tests::restored_reopen().await;
    fixture.backend.hold_reopen.store(true, Ordering::SeqCst);
    let worker = fixture
        .harness
        .worker_with_adapter("stalled-reopen", fixture.adapter(&runtime_image()));
    let id = reopening.id.clone();
    now += Duration::from_secs(6);
    let task = tokio::spawn(async move { worker.run_one(&id, now).await });
    tokio::time::timeout(
        Duration::from_secs(3),
        fixture.backend.reopen_entered.notified(),
    )
    .await
    .unwrap();
    let competing = fixture
        .harness
        .worker_with_adapter("competitor", fixture.adapter(&runtime_image()));
    assert!(!competing.run_one(&reopening.id, now).await.unwrap());
    let owner = caller("owner", [Capability::DeleteOwnSandbox]);
    fixture
        .harness
        .service
        .delete(&owner, &reopening.id)
        .await
        .unwrap();
    fixture.backend.reopen_release.notify_one();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap(),
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        fixture
            .harness
            .store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Paused
    );
    retire_and_prove(&fixture, &reopening, &agent, now).await;
}
