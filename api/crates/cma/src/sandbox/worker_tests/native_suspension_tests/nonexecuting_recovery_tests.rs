use super::*;
use layer2_core::profiles::WorkspaceProfileSnapshot;
use layer2_core::sandbox::store::{SandboxReconciliationStore, SandboxRuntimeLossRequest};
use layer2_core::sandbox::{
    SandboxUnpublishedRecoveryRequest, SandboxUnpublishedRecoveryVerifier, Workspace,
    WorkspaceOrigin, WorkspaceSource, WorkspaceStatus, WorkspaceWorkState, WorkspaceWorkStep,
};

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
            automatic: false,
            requested_at: stamp,
        })
        .await
        .unwrap();
    assert_eq!(next.lifecycle.home, active.lifecycle.home);
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
