use super::*;
use layer2_core::sandbox::store::{
    SandboxReconciliationStore, SandboxRuntimeLossOutcome, SandboxRuntimeLossRequest,
};

pub(super) async fn active_fixture() -> (Fixture, Sandbox, SystemTime) {
    let mut fixture = Fixture::new().await;
    let created = fixture.harness.create().await;
    let mut now = start_time(&created);
    let active = fixture
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    fixture.harness.adapter = fixture.adapter(&runtime_image());
    crate::sandbox::runtime_attachment::adopt_bound_runtime(
        fixture.harness.adapter.as_ref(),
        &active,
        "test",
    )
    .await
    .unwrap();
    (fixture, active, now)
}

pub(super) async fn reconcile(fixture: &Fixture, now: SystemTime) {
    SandboxReconciler::new(
        fixture.harness.store.clone(),
        fixture.adapter(&runtime_image()),
        Arc::new(fixture.harness.policy.clone()),
        "test".into(),
    )
    .initialize(now)
    .await
    .unwrap();
}

pub(super) async fn after_durable_boundary(sandbox: &Sandbox) -> SystemTime {
    let durable = stamp_time(&sandbox.updated_at);
    if let Ok(delay) = durable
        .duration_since(SystemTime::now())
        .map(|delay| delay + Duration::from_millis(10))
    {
        tokio::time::sleep(delay).await;
    }
    SystemTime::now()
}

#[tokio::test]
async fn process_loss_retains_object_home_and_conversations_across_every_worker_restart() {
    let (fixture, active, mut now) = active_fixture().await;
    let agent = lifecycle_recovery_tests::prepare_agent(&fixture.harness, &active, now, 1).await;
    let original = fixture.backend.snapshot().await;
    fixture.backend.restart_process().await;
    now = after_durable_boundary(&active).await;
    reconcile(&fixture, now).await;
    let recovering = fixture.harness.stored(&active.id).await;
    assert_eq!(recovering.status, SandboxStatus::Reviving);
    assert!(recovering.runtime.is_none());
    assert_eq!(
        recovering.lifecycle.suspended_runtime.as_deref(),
        active.runtime.as_ref()
    );
    assert_eq!(recovering.lifecycle.home, active.lifecycle.home);
    let interruption = recovering.lifecycle.interruption.as_ref().unwrap();
    assert_eq!(
        interruption.cause,
        layer2_core::sandbox::SandboxInterruptionCause::RuntimeUnavailable
    );
    assert_eq!(interruption.detected_at, rfc3339_at(now));
    assert_eq!(
        interruption.memory_limit_mib,
        active.profile.content.memory_mib
    );
    let persisted = serde_json::to_vec(&recovering).unwrap();
    assert_eq!(
        serde_json::from_slice::<Sandbox>(&persisted).unwrap(),
        recovering
    );

    fixture.backend.lose_suspend.store(true, Ordering::SeqCst);
    fixture.backend.lose_resume.store(true, Ordering::SeqCst);
    let resumed = fixture
        .drive(
            &active.id,
            &mut now,
            SandboxStatus::Active,
            "changed-default-image",
        )
        .await;
    let before = active.runtime.as_ref().unwrap();
    let after = resumed.runtime.as_ref().unwrap();
    assert_eq!(after.runtime_id, before.runtime_id);
    assert_eq!(after.runtime_version, before.runtime_version);
    assert!(after.provision_generation > before.provision_generation);
    assert_ne!(after.boot_id, before.boot_id);
    assert_eq!(resumed.lifecycle.home, active.lifecycle.home);
    assert_eq!(
        resumed.lifecycle.interruption,
        recovering.lifecycle.interruption
    );
    assert!(resumed.lifecycle.suspended_runtime.is_none());
    let snapshot = fixture.backend.snapshot().await;
    assert_eq!(snapshot.workspaces, original.workspaces);
    assert_eq!(snapshot.worktrees, original.worktrees);
    assert_eq!(snapshot.homes, original.homes);
    let restored = fixture
        .harness
        .store
        .get_agent(&agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        restored.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert_eq!(restored.status, AgentStatus::Ready);
    let calls = fixture.backend.calls.snapshot();
    for operation in ["create", "suspend", "resume"] {
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.as_str() == operation)
                .count(),
            1,
            "{calls:?}"
        );
    }
    assert!(calls.contains(&"resume-replay".into()));
    assert!(!calls.contains(&"delete-retained".into()));
    // NativeBackend::reclaim panics: recovery may not delete the original CRD.
}

#[tokio::test]
async fn older_recovery_without_retained_binding_fails_without_reclaiming_original_home() {
    let (fixture, active, mut now) = active_fixture().await;
    let request = SandboxRuntimeLossRequest {
        sandbox_id: active.id.clone(),
        expected_revision: active.revision,
        expected_runtime: active.runtime.clone(),
        native_retirement_finality: false,
        retain_runtime: false,
        interruption_cause: Default::default(),
        observed_at: rfc3339_at(now),
    };
    assert_eq!(
        fixture
            .harness
            .store
            .recover_runtime_loss(request)
            .await
            .unwrap(),
        SandboxRuntimeLossOutcome::RecoveryScheduled
    );
    // A previous worker can have advanced its generation reservation before
    // restart. The inventory sweeper must still protect the older home owner.
    let stored = fixture.harness.stored(&active.id).await;
    let mut reserved = stored.clone();
    reserved.lifecycle.runtime_generation_high_water += 1;
    reserved.revision += 1;
    fixture
        .harness
        .store
        .save_sandbox(stored.revision, reserved)
        .await
        .unwrap();
    now += Duration::from_secs(6);
    fixture.step(&active.id, now, &runtime_image()).await;
    let failed = fixture.harness.stored(&active.id).await;
    assert_eq!(failed.status, SandboxStatus::Failed);
    assert_eq!(failed.lifecycle.home, active.lifecycle.home);
    assert!(!failed.lifecycle.home.as_ref().unwrap().released);
    assert_eq!(fixture.backend.calls.snapshot(), ["create"]);
}

#[tokio::test]
async fn stale_loss_observation_and_worker_cannot_suspend_a_newer_recovery() {
    let (fixture, active, _initial_now) = active_fixture().await;
    fixture.backend.restart_process().await;
    let now = after_durable_boundary(&active).await;
    reconcile(&fixture, now).await;
    let stale_loss = SandboxRuntimeLossRequest {
        sandbox_id: active.id.clone(),
        expected_revision: active.revision,
        expected_runtime: active.runtime.clone(),
        native_retirement_finality: false,
        retain_runtime: true,
        interruption_cause: Default::default(),
        observed_at: rfc3339_at(now),
    };
    assert_eq!(
        fixture
            .harness
            .store
            .recover_runtime_loss(stale_loss)
            .await
            .unwrap(),
        SandboxRuntimeLossOutcome::Superseded
    );
    let expires = now + Duration::from_secs(2);
    let stale = fixture
        .harness
        .store
        .claim_sandbox_work(
            &active.id,
            "old-worker",
            rfc3339_at(now),
            rfc3339_at(expires),
        )
        .await
        .unwrap()
        .unwrap();
    let worker = fixture
        .harness
        .worker_with_adapter("new-worker", fixture.adapter(&runtime_image()));
    worker.run_due(expires).await.unwrap();
    let current = fixture.harness.stored(&active.id).await;
    let calls = fixture.backend.calls.snapshot();
    assert!(worker
        .execute_claimed_for_test(stale, expires)
        .await
        .is_err());
    assert_eq!(fixture.harness.stored(&active.id).await, current);
    assert_eq!(fixture.backend.calls.snapshot(), calls);
}

#[tokio::test]
async fn missing_original_home_or_provider_object_never_creates_an_empty_replacement() {
    for lose_home in [false, true] {
        let (fixture, active, _initial_now) = active_fixture().await;
        fixture.backend.restart_process().await;
        let now = after_durable_boundary(&active).await;
        reconcile(&fixture, now).await;
        if lose_home {
            fixture.homes.lose_disk();
        } else {
            fixture.backend.lose_object().await;
        }
        fixture
            .step(&active.id, now + Duration::from_secs(6), &runtime_image())
            .await;
        let stopped = fixture.harness.stored(&active.id).await;
        assert_ne!(stopped.status, SandboxStatus::Active);
        assert_eq!(stopped.lifecycle.home, active.lifecycle.home);
        assert_eq!(
            stopped.lifecycle.suspended_runtime.as_deref(),
            active.runtime.as_ref()
        );
        assert_eq!(fixture.backend.calls.snapshot(), ["create"]);
    }
}

#[path = "fair_recovery_tests.rs"]
mod fair_recovery_tests;

#[path = "binding_opportunity_tests.rs"]
mod binding_opportunity_tests;

#[path = "binding_authority_tests.rs"]
mod binding_authority_tests;
