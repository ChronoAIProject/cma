//! Drive the admitted state through the existing retained-home worker suffix.
use super::*;
use layer2_core::sandbox::store::{AgentLogStore, SandboxEventStore};
use layer2_core::sandbox::{
    RetainedReopenRequest, RetainedReopenVerifier, SandboxEventKind, TypedFailure,
};

pub(super) async fn restored_reopen() -> (Fixture, Sandbox, layer2_core::sandbox::Agent, SystemTime)
{
    restored_reopen_after_pause(false).await
}

pub(super) async fn restored_reopen_after_pause(
    inactivity: bool,
) -> (Fixture, Sandbox, layer2_core::sandbox::Agent, SystemTime) {
    restored_reopen_with_settings(inactivity, None).await
}

pub(super) async fn restored_reopen_with_settings(
    inactivity: bool,
    settings: Option<layer2_core::sandbox::AgentSettings>,
) -> (Fixture, Sandbox, layer2_core::sandbox::Agent, SystemTime) {
    let mut fixture = Fixture::native().await;
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
    // Native history proof requires the provider's UUID conversation format.
    let agent = lifecycle_recovery_tests::prepare_agent_with_conversation(
        &fixture.harness,
        &active,
        now,
        1,
        Some(uuid::Uuid::new_v4().to_string()),
        settings,
    )
    .await;
    fixture
        .harness
        .store
        .create_workspace(lifecycle_recovery_tests::workspace_for(&agent, now))
        .await
        .unwrap();
    let paused = if inactivity {
        use layer2_core::sandbox::store::{
            InactivityPauseCause, SandboxInactivityPauseRequest, SandboxMaintenanceOutcome,
            SandboxMaintenanceStore,
        };
        now += Duration::from_secs(15 * 60 + 1);
        let admitted_at = now;
        let wall_clock = std::time::Instant::now();
        assert_eq!(
            fixture
                .harness
                .store
                .schedule_inactivity_pause(SandboxInactivityPauseRequest {
                    sandbox_id: active.id.clone(),
                    requested_at: rfc3339_at(now),
                    pause_after_seconds: 15 * 60,
                    auto_pause_enabled: true,
                    expected_policy_revision: None,
                    cause: InactivityPauseCause::Inactivity,
                })
                .await
                .unwrap(),
            SandboxMaintenanceOutcome::Scheduled,
        );
        assert_eq!(
            fixture.harness.stored(&active.id).await.status,
            SandboxStatus::Pausing
        );
        let paused = fixture
            .drive(
                &active.id,
                &mut now,
                SandboxStatus::Paused,
                &runtime_image(),
            )
            .await;
        assert_eq!(paused.lifecycle.home, active.lifecycle.home);
        eprintln!(
            "idle=901s; Pausing -> Paused logical={}s wall={}ms",
            now.duration_since(admitted_at).unwrap().as_secs(),
            wall_clock.elapsed().as_millis()
        );
        paused
    } else {
        fixture.pause(&active, &mut now).await
    };
    let revive_at = now;
    let wall_clock = std::time::Instant::now();
    fixture
        .harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            paused.id.clone(),
            paused.owner.clone(),
            rfc3339_at(now),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    for _ in 0..10 {
        let current = fixture.harness.stored(&active.id).await;
        if matches!(
            current.lifecycle.operation,
            Some(SandboxLifecycleOperation::Revive {
                phase: SandboxRevivePhase::ConversationReopen,
                ..
            })
        ) {
            break;
        }
        now += Duration::from_secs(6);
        fixture.step(&active.id, now, &runtime_image()).await;
    }
    let current = fixture.harness.stored(&active.id).await;
    assert!(matches!(
        current.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            phase: SandboxRevivePhase::ConversationReopen,
            ..
        })
    ));
    if inactivity {
        eprintln!(
            "Reviving -> ConversationReopen logical={}s wall={}ms",
            now.duration_since(revive_at).unwrap().as_secs(),
            wall_clock.elapsed().as_millis()
        );
    }
    let child = fixture
        .harness
        .store
        .get_agent(&agent.id)
        .await
        .unwrap()
        .unwrap();
    (fixture, current, child, now)
}

async fn restored_failure() -> (Fixture, Sandbox, layer2_core::sandbox::Agent, SystemTime) {
    let (fixture, current, before, now) = restored_reopen().await;
    let mut failed = current.clone();
    failed.status = SandboxStatus::Failed;
    failed.failure = Some(TypedFailure {
        class: "restore_failed".into(),
        detail: "runtime rejected the lifecycle operation".into(),
    });
    failed.lifecycle.operation = None;
    failed.lifecycle.home_recovery = None;
    failed.work.schedule(SandboxWorkStep::None, rfc3339_at(now));
    failed.revision += 1;
    failed.validate().unwrap();
    fixture
        .harness
        .store
        .save_sandbox(current.revision, failed.clone())
        .await
        .unwrap();
    let child = fixture
        .harness
        .store
        .get_agent(&before.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(child.status, AgentStatus::Paused);
    assert!(
        child.conversation.runtime_generation
            < failed.runtime.as_ref().unwrap().provision_generation
    );
    (fixture, failed, child, now)
}

async fn request(fixture: &Fixture, failed: &Sandbox, now: SystemTime) -> RetainedReopenRequest {
    let snapshot = fixture
        .harness
        .store
        .retained_reopen_snapshot(&failed.id)
        .await
        .unwrap();
    snapshot.validate().unwrap();
    let binding = failed.runtime.as_ref().unwrap();
    let source = snapshot.retained.source().unwrap();
    let histories = snapshot.retained.agents.iter().map(|a| serde_json::json!({
        "agentId": a.id.as_str(), "conversationId": a.conversation.conversation_id.as_ref().unwrap(), "historySha256": "b".repeat(64),
    })).collect::<Vec<_>>();
    *fixture.backend.recovery_plan.lock().unwrap() = serde_json::json!({
        "idleLedger": {"format":"legacy_v1_unused_process_scope", "sha256":"a".repeat(64), "journals":2, "authorities":2},
        "idleReopen": true,
        "runtime": {"sandboxId": failed.id.as_str(), "generation": source.generation,
            "homeIdentity": source.home_identity, "bootId": binding.boot_id.as_ref().unwrap()},
        "agents": snapshot.retained.agents.iter().map(|a| serde_json::json!({
            "agentId": a.id.as_str(), "conversationId": a.conversation.conversation_id.as_ref().unwrap(),
        })).collect::<Vec<_>>(), "histories": histories,
    });
    let verifier = crate::sandbox::retained_recovery::RetainedRuntimeInspector::new(
        fixture.adapter(&runtime_image()),
        "test".into(),
    );
    let proof = verifier.verify(&snapshot).await.unwrap();
    RetainedReopenRequest {
        repeat_authorized: false,
        snapshot,
        proof,
        key_sha256: "e".repeat(64),
        requested_at: rfc3339_at(now),
    }
}

#[tokio::test]
async fn retained_reopen_restarts_each_worker_phase_without_changing_home_or_replaying_input() {
    let (fixture, failed, before, mut now) = restored_failure().await;
    let home = fixture.backend.snapshot().await;
    let request = request(&fixture, &failed, now).await;
    let call_count = fixture.backend.calls.snapshot().len();
    let accepted = fixture
        .harness
        .store
        .admit_retained_reopen(request.clone())
        .await
        .unwrap();
    for _ in 0..3 {
        now += Duration::from_secs(6);
        // A fresh adapter and reconciler on every phase exercises API restart.
        fixture.step(&failed.id, now, &runtime_image()).await;
    }
    let after = fixture.harness.stored(&failed.id).await;
    assert_eq!(after.status, SandboxStatus::Active, "{:?}", after.failure);
    assert_eq!(after.runtime, failed.runtime);
    assert_eq!(after.lifecycle.home, failed.lifecycle.home);
    let child = fixture
        .harness
        .store
        .get_agent(&before.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(child.id, before.id);
    assert_eq!(child.worktree_id, before.worktree_id);
    assert_eq!(
        child.conversation.conversation_id,
        before.conversation.conversation_id
    );
    assert_eq!(child.status, AgentStatus::Ready);
    assert_eq!(
        child.conversation.runtime_generation,
        after.runtime.as_ref().unwrap().provision_generation
    );
    let native = fixture.backend.snapshot().await;
    assert_eq!(native.homes, home.homes);
    assert_eq!(native.worktrees, home.worktrees);
    assert!(fixture.backend.inputs.lock().unwrap().is_empty());
    assert_eq!(
        fixture
            .harness
            .store
            .admit_retained_reopen(request)
            .await
            .unwrap(),
        accepted
    );
    let events = fixture
        .harness
        .store
        .sandbox_events(&failed.id, 0, 100)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.kind,
                SandboxEventKind::StatusChanged {
                    from: SandboxStatus::Failed,
                    to: SandboxStatus::Reviving
                }
            ))
            .count(),
        1
    );
    let logs = fixture
        .harness
        .store
        .agent_log(&child.id, 0, 100)
        .await
        .unwrap();
    assert!(!logs.iter().any(|entry| matches!(
        entry.kind,
        layer2_core::sandbox::AgentLogKind::OwnerMessage { .. }
    )));
    assert!(!fixture
        .backend
        .calls
        .snapshot()
        .iter()
        .skip(call_count)
        .any(|c| matches!(
            c.as_str(),
            "suspend" | "resume" | "upgrade" | "delete-retained" | "create"
        )));
}

#[tokio::test]
async fn retained_reopen_replacement_before_each_suffix_phase_never_publishes_active_or_replays() {
    for completed_phases in 0..3 {
        let (fixture, failed, before, mut now) = restored_failure().await;
        let request = request(&fixture, &failed, now).await;
        fixture
            .harness
            .store
            .admit_retained_reopen(request)
            .await
            .unwrap();
        for _ in 0..completed_phases {
            now += Duration::from_secs(6);
            fixture.step(&failed.id, now, &runtime_image()).await;
        }
        let published = fixture
            .harness
            .store
            .get_agent(&before.id)
            .await
            .unwrap()
            .unwrap();
        fixture.backend.restart_process().await;
        now += Duration::from_secs(6);
        fixture.step(&failed.id, now, &runtime_image()).await;
        let after = fixture.harness.stored(&failed.id).await;
        assert_ne!(
            after.status,
            SandboxStatus::Active,
            "phase {completed_phases}"
        );
        let child = fixture
            .harness
            .store
            .get_agent(&before.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            child.conversation.conversation_id,
            before.conversation.conversation_id
        );
        assert_eq!(
            child.conversation.runtime_generation,
            published.conversation.runtime_generation
        );
        assert!(fixture.backend.inputs.lock().unwrap().is_empty());
    }
}
