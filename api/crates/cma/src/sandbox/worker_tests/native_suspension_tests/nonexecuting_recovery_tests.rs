use super::*;
use layer2_core::profiles::WorkspaceProfileSnapshot;
use layer2_core::sandbox::store::{SandboxReconciliationStore, SandboxRuntimeLossRequest};
use layer2_core::sandbox::work_retry::WORK_RETRY_EXTERNAL_BUDGET;
use layer2_core::sandbox::{
    SandboxLifecycleResult, SandboxRuntimeUpdateService, SandboxUnpublishedRecoveryRequest,
    SandboxUnpublishedRecoveryVerifier, Workspace, WorkspaceOrigin, WorkspaceSource,
    WorkspaceStatus, WorkspaceWorkState, WorkspaceWorkStep,
};
use layer2_core::{Authorization, Caller, Capability};

const TARGET: &str = "registry.example/runtime@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

async fn advance_fixture(f: &Fixture, id: &SandboxId, now: SystemTime, phase: usize) {
    let claimed = f
        .harness
        .store
        .claim_sandbox_work(
            id,
            "old-worker",
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    let lease = claimed.work.lease.as_ref().unwrap();
    let mut next = claimed.clone();
    match phase {
        0 => next
            .complete_lifecycle_home_verification(
                claimed.retained_home_recovery().unwrap(),
                &lease.holder,
                lease.fence,
                rfc3339_at(now),
            )
            .unwrap(),
        1 => {
            next.reserve_lifecycle_runtime_generation(
                2,
                &lease.holder,
                lease.fence,
                rfc3339_at(now),
            )
            .unwrap();
        }
        _ => next
            .retry_lifecycle(rfc3339_at(now), &lease.holder, lease.fence, rfc3339_at(now))
            .unwrap(),
    }
    f.harness
        .store
        .save_sandbox(claimed.revision, next)
        .await
        .unwrap();
}

async fn admitted() -> (Fixture, Sandbox, Workspace, SystemTime) {
    admitted_generation(None).await
}

#[tokio::test]
async fn refused_quiescence_parks_same_runtime_update_and_owner_retry_reuses_its_proof() {
    let (fixture, admitted, _, mut now) = admitted().await;
    fixture.backend.nonexecuting.store(false, Ordering::SeqCst);
    for attempt in 0..=WORK_RETRY_EXTERNAL_BUDGET {
        let scheduled = fixture.harness.stored(&admitted.id).await;
        now = now.max(stamp_time(&scheduled.work.ready_at));
        fixture.step(&admitted.id, now, TARGET).await;
        let current = fixture.harness.stored(&admitted.id).await;
        assert_eq!(current.lifecycle.home, admitted.lifecycle.home);
        assert_eq!(
            current.lifecycle.suspended_runtime,
            admitted.lifecycle.suspended_runtime
        );
        assert_eq!(
            current.lifecycle.unpublished_recovery,
            admitted.lifecycle.unpublished_recovery
        );
        assert_eq!(current.lifecycle.operation, admitted.lifecycle.operation);
        assert!(current.runtime.is_none());
        assert!(current.work.lease.is_none());
        if attempt < WORK_RETRY_EXTERNAL_BUDGET {
            assert_eq!(current.work.step, SandboxWorkStep::RunLifecycleOperation);
            assert_eq!(current.work.attempts, attempt + 1);
        }
    }
    let parked = fixture.harness.stored(&admitted.id).await;
    assert_eq!(parked.work.step, SandboxWorkStep::None);
    assert_eq!(
        parked
            .runtime_update_request()
            .unwrap()
            .failure
            .as_ref()
            .unwrap()
            .class,
        "terminal_validation"
    );
    let result = SandboxLifecycleResult::observe(&parked, None, now, true).unwrap();
    assert_eq!(result.code, Some("runtime_update_parked"));
    assert!(!result.continuing);
    assert!(
        !fixture.backend.calls.snapshot().iter().any(|call| matches!(
            call.as_str(),
            "suspend" | "resume" | "upgrade" | "delete-retained"
        ))
    );

    let retried = retry_parked_update(&fixture, &parked).await;
    assert_eq!(retried.work.step, SandboxWorkStep::RunLifecycleOperation);
    assert_eq!(retried.work.attempts, 0);
    assert!(retried.runtime_update_request().unwrap().failure.is_none());
    assert_eq!(retried.lifecycle.operation, parked.lifecycle.operation);
    assert_eq!(
        retried.lifecycle.unpublished_recovery,
        parked.lifecycle.unpublished_recovery
    );
}

async fn retry_parked_update(fixture: &Fixture, parked: &Sandbox) -> Sandbox {
    let service = SandboxRuntimeUpdateService::new(fixture.harness.store.clone(), TARGET.into());
    let caller = Caller {
        subject: parked.owner.clone(),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    service.request(&caller, &parked.id, TARGET).await.unwrap()
}

#[path = "quiescence_recovery_tests.rs"]
mod quiescence_recovery_tests;

pub(super) async fn stalled(
    provider_generation: Option<u64>,
) -> (
    Fixture,
    layer2_core::sandbox::SandboxUnpublishedRecoverySnapshot,
    Workspace,
    SystemTime,
) {
    let f = Fixture::native().await;
    let created = f.harness.create().await;
    let mut now = start_time(&created);
    let active = f
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    let stamp = rfc3339_at(now);
    let workspace = Workspace {
        id: layer2_core::WorkspaceId::generate(),
        sandbox_id: active.id.clone(),
        owner: active.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId::generate(),
            revision: 1,
            digest: "a".repeat(64),
            content: WorkspaceProfileContent {
                name: "project".into(),
                location: WorkspaceLocation::Directory {
                    name: "project".into(),
                },
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: "project".into(),
        path: "/home/cma/project".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: None,
        status: WorkspaceStatus::Preparing,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, stamp.clone()),
        next_worktree_number: 1,
        created_at: stamp.clone(),
        ready_at: None,
        failed: None,
        removed_at: None,
        revision: 1,
    };
    f.harness
        .store
        .create_workspace(workspace.clone())
        .await
        .unwrap();
    let active = f.harness.stored(&active.id).await;
    f.harness
        .store
        .recover_runtime_loss(SandboxRuntimeLossRequest {
            sandbox_id: active.id.clone(),
            expected_revision: active.revision,
            expected_runtime: active.runtime.clone(),
            native_retirement_finality: false,
            retain_runtime: true,
            interruption_cause: Default::default(),
            observed_at: stamp.clone(),
        })
        .await
        .unwrap();
    // Reproduce the old persisted revival: source generation 1 remains in its
    // original provider object while generation 2 is reserved but never applied.
    for phase in 0..3 {
        advance_fixture(&f, &active.id, now, phase).await;
    }
    f.backend.nonexecuting.store(true, Ordering::SeqCst);
    let snapshot = f
        .harness
        .store
        .unpublished_runtime_recovery_snapshot(&active.id)
        .await
        .unwrap();
    assert!(snapshot.agents.is_empty());
    assert_eq!(snapshot.runtime_generation().unwrap(), 1);
    assert_eq!(snapshot.sandbox.lifecycle.runtime_generation_high_water, 2);
    if let Some(generation) = provider_generation {
        let spec =
            crate::sandbox::runtime_mapping::sandbox_spec(&snapshot.sandbox, "test").unwrap();
        let request = CodexSandboxSpec::default()
            .deployment
            .render_runtime(&spec, generation)
            .unwrap();
        f.backend.stage_reserved_request(request).await;
    }
    (f, snapshot, workspace, now)
}

pub(super) async fn admitted_generation(
    provider_generation: Option<u64>,
) -> (Fixture, Sandbox, Workspace, SystemTime) {
    let (f, snapshot, workspace, now) = stalled(provider_generation).await;
    let original_home = snapshot.sandbox.lifecycle.home.clone();
    let verifier = crate::sandbox::unpublished_recovery::UnpublishedRuntimeRecovery::new(
        f.adapter(TARGET),
        "test".into(),
    );
    let proof = verifier.verify(&snapshot).await.unwrap();
    assert!(proof.boot_id.is_none());
    let next = f
        .harness
        .store
        .admit_unpublished_runtime_recovery(SandboxUnpublishedRecoveryRequest {
            snapshot,
            proof,
            target_runtime_version: TARGET.into(),
            released_runtime: None,
            automatic: false,
            requested_at: rfc3339_at(now),
        })
        .await
        .unwrap();
    assert_eq!(next.lifecycle.home, original_home);
    assert_eq!(next.lifecycle.runtime_generation_high_water, 2);
    (f, next, workspace, now)
}

#[tokio::test]
async fn no_agent_crashloop_recovery_keeps_original_home_pending_workspace_and_reserved_floor() {
    let (f, admitted, workspace, mut now) = admitted().await;
    now += Duration::from_secs(6);
    f.step(&admitted.id, now, TARGET).await;
    let recorded = f.harness.stored(&admitted.id).await;
    assert!(recorded
        .lifecycle
        .unpublished_recovery
        .as_ref()
        .unwrap()
        .quiescence
        .is_some());
    assert!(!f.backend.calls.snapshot().contains(&"suspend".into()));
    f.backend.lose_suspend.store(true, Ordering::SeqCst);
    f.backend.lose_resume.store(true, Ordering::SeqCst);
    let restored = f
        .drive(&admitted.id, &mut now, SandboxStatus::Active, TARGET)
        .await;
    assert_eq!(restored.lifecycle.home, admitted.lifecycle.home);
    let runtime = restored.runtime.as_ref().unwrap();
    assert_eq!(
        runtime.runtime_id,
        admitted
            .lifecycle
            .suspended_runtime
            .as_ref()
            .unwrap()
            .runtime_id
    );
    assert_eq!(runtime.provision_generation, Some(3));
    assert_eq!(runtime.runtime_version, TARGET);
    assert_eq!(
        f.harness
            .store
            .get_workspace(&workspace.id)
            .await
            .unwrap()
            .unwrap(),
        workspace
    );
    let calls = f.backend.calls.snapshot();
    let suspend = calls.iter().position(|call| call == "suspend").unwrap();
    assert_eq!(calls[suspend - 1], "inspect-nonexecuting");
    assert!(calls.contains(&"verify-suspended".into()));
    assert!(calls.contains(&"resume-replay".into()));
    assert!(!calls.contains(&"delete-retained".into()));
    assert_eq!(calls.iter().filter(|call| *call == "create").count(), 1);
}

#[tokio::test]
async fn changed_or_running_container_after_receipt_never_authorizes_suspension() {
    for running in [false, true] {
        let (f, admitted, workspace, mut now) = admitted().await;
        now += Duration::from_secs(6);
        f.step(&admitted.id, now, TARGET).await;
        if running {
            f.backend.run_before_suspend.store(true, Ordering::SeqCst);
        } else {
            f.backend
                .change_before_suspend
                .store(true, Ordering::SeqCst);
        }
        now += Duration::from_secs(6);
        f.step(&admitted.id, now, TARGET).await;
        let after = f.harness.stored(&admitted.id).await;
        assert_eq!(after.lifecycle.home, admitted.lifecycle.home);
        assert_eq!(
            after.lifecycle.suspended_runtime,
            admitted.lifecycle.suspended_runtime
        );
        assert!(after.runtime.is_none());
        assert!(!f.backend.calls.snapshot().contains(&"suspend".into()));
        assert_eq!(
            f.harness
                .store
                .get_workspace(&workspace.id)
                .await
                .unwrap()
                .unwrap(),
            workspace
        );
        // A later safe observation may refresh the stopped witness, but retains
        // the original admitted runtime/request/home and generation floor.
        f.backend.nonexecuting.store(true, Ordering::SeqCst);
        let restored = f
            .drive(&admitted.id, &mut now, SandboxStatus::Active, TARGET)
            .await;
        assert_eq!(restored.lifecycle.home, admitted.lifecycle.home);
        assert_eq!(
            restored.runtime.as_ref().unwrap().provision_generation,
            Some(3)
        );
    }
}

#[tokio::test]
async fn recovered_sandbox_serves_while_interrupted_workspace_parks_terminally() {
    use crate::sandbox::workspace_worker::{
        WorkspaceWorker, WorkspaceWorkerConfig, WorkspaceWorkerDependencies,
    };
    let (f, admitted, workspace, mut now) = admitted().await;
    let active = f
        .drive(&admitted.id, &mut now, SandboxStatus::Active, TARGET)
        .await;
    f.backend
        .interrupted_workspace
        .store(true, Ordering::SeqCst);
    let worker = WorkspaceWorker::new(
        WorkspaceWorkerDependencies {
            store: f.harness.store.clone(),
            adapter: f.adapter(TARGET),
        },
        WorkspaceWorkerConfig::new("workspace-after-recovery", "test"),
    );
    now += Duration::from_secs(6);
    assert!(worker.run_one(&workspace.id, now).await.unwrap());
    let failed = f
        .harness
        .store
        .get_workspace(&workspace.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.status, WorkspaceStatus::Failed);
    assert_eq!(failed.work.step, WorkspaceWorkStep::None);
    assert!(failed.work.lease.is_none());
    assert_eq!(
        failed.failed.as_ref().unwrap().class,
        "workspace_runtime_rejected"
    );
    assert!(!worker
        .run_one(&workspace.id, now + Duration::from_secs(60))
        .await
        .unwrap());
    assert_eq!(
        f.harness.stored(&active.id).await.status,
        SandboxStatus::Active
    );
    assert_eq!(
        f.backend
            .calls
            .snapshot()
            .iter()
            .filter(|call| *call == "workspace-indeterminate")
            .count(),
        1
    );
}

#[tokio::test]
async fn nonexecuting_refresh_refuses_changed_admitted_source_or_plan_digest() {
    let (f, admitted, _, _) = admitted().await;
    let proof = &admitted
        .lifecycle
        .unpublished_recovery
        .as_ref()
        .unwrap()
        .proof;
    let source = crate::sandbox::runtime_attachment::retained_runtime_request(
        &admitted,
        admitted.lifecycle.suspended_runtime.as_deref().unwrap(),
        "test",
    )
    .unwrap();
    let original = layer3_adapter::NonexecutingRecoveryVerification {
        source,
        provider_plan: proof.provider_plan.clone(),
        provider_plan_sha256: proof.provider_plan_sha256.clone(),
    };
    let adapter = f.adapter(TARGET);
    let recovery = adapter.runtime_recovery().unwrap();
    for field in ["handle", "request_sha256", "home_identity", "unknown_field"] {
        let mut changed = original.clone();
        changed.provider_plan[field] = serde_json::json!("changed");
        changed.provider_plan_sha256 =
            layer2_core::sandbox::SandboxUnpublishedRecoveryProof::provider_plan_digest(
                &changed.provider_plan,
            )
            .unwrap();
        assert!(
            recovery
                .verify_nonexecuting_recovery(&changed)
                .await
                .is_err(),
            "{field}"
        );
    }
    let mut changed = original;
    changed.provider_plan_sha256 = "0".repeat(64);
    assert!(recovery
        .verify_nonexecuting_recovery(&changed)
        .await
        .is_err());
    assert!(!f.backend.calls.snapshot().contains(&"suspend".into()));
}
