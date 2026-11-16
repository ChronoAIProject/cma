use super::*;

#[tokio::test]
async fn create_installs_before_credential_then_builds_workspaces_in_order() {
    let harness = Harness::new(
        vec!["apt-get update".into(), "apt-get install -y git".into()],
        2,
        Duration::from_secs(60),
    )
    .await;
    let created = harness.create().await;
    assert_eq!(created.status, SandboxStatus::Provisioning);
    let active = harness
        .drive_active(&created.id, start_time(&created))
        .await;

    assert_eq!(active.status, SandboxStatus::Active);
    assert_eq!(active.work.step, SandboxWorkStep::WaitForCredentialRotation);
    assert_eq!(active.credential.key_generation, 1);
    assert!(active.credential.current.is_some());
    assert!(active.credential.pending.is_none());
    assert!(harness
        .credentials
        .material(&created.id, 1)
        .await
        .unwrap()
        .is_some());
    let requests = harness.backend.requests().await;
    assert!(matches!(&requests[0].1, RequestFrame::Install(request)
        if request.commands == ["apt-get update", "apt-get install -y git"]));
    assert!(matches!(&requests[1].1, RequestFrame::Credential(frame)
        if frame.value.expose() == "one-time-secret-1"));
    assert_eq!(
        harness.actions.snapshot(),
        [
            "mint:1",
            "workspace:0:wkp_worker_0",
            "workspace:1:wkp_worker_1",
        ]
    );
    let binding = active.runtime.as_ref().unwrap();
    let runtime = harness
        .backend
        .snapshot(&RuntimeHandle(binding.runtime_id.clone()))
        .await
        .unwrap();
    assert_eq!(runtime.request.env["PROJECT_MODE"], "integration");
    assert_eq!(
        runtime.request.env["NYXID_ACCESS_TOKEN"],
        layer3_adapter::hosted::PLACEHOLDER_CREDENTIAL
    );
    assert!(runtime.credential_delivered);

    let events = harness
        .store
        .sandbox_events(&created.id, 0, 20)
        .await
        .unwrap();
    assert_eq!(events.len(), 8);
    assert_eq!(events[0].kind, SandboxEventKind::Created);
    assert_eq!(
        events[1].kind,
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Provisioning,
            to: SandboxStatus::Installing,
        }
    );
    assert_eq!(
        events[2].kind,
        SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::RunInstall,
        }
    );
    assert_eq!(
        events[3].kind,
        SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::MintInitialCredential,
        }
    );
    assert_eq!(
        events[4].kind,
        SandboxEventKind::CredentialChanged { key_generation: 1 }
    );
    assert_eq!(
        events[5].kind,
        SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::CreateProfileWorkspace(0),
        }
    );
    assert_eq!(
        events[6].kind,
        SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::CreateProfileWorkspace(1),
        }
    );
    assert_eq!(
        events[7].kind,
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Installing,
            to: SandboxStatus::Active,
        }
    );
}

#[tokio::test]
async fn worker_pauses_and_revives_a_sandbox_from_its_held_checkpoint() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let old_runtime = RuntimeHandle(active.runtime.as_ref().unwrap().runtime_id.clone());
    let pause_at = now + Duration::from_secs(1);
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(pause_at),
            Some("inactive".into()),
        ))
        .await
        .unwrap();

    for _ in 0..4 {
        harness.run(&active.id, pause_at).await;
    }
    let paused = harness.stored(&active.id).await;
    assert_eq!(paused.status, SandboxStatus::Paused);
    assert!(paused.runtime.is_none());
    assert!(paused.lifecycle.operation.is_none());
    assert_eq!(
        paused
            .lifecycle
            .checkpoints
            .latest
            .as_ref()
            .unwrap()
            .conversations,
        []
    );
    assert!(harness.backend.snapshot(&old_runtime).await.is_err());

    let revive_at = pause_at + Duration::from_secs(1);
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            paused.id.clone(),
            paused.owner.clone(),
            rfc3339_at(revive_at),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    for _ in 0..8 {
        harness.run(&paused.id, revive_at).await;
    }
    let revived = harness.stored(&paused.id).await;
    assert_eq!(revived.status, SandboxStatus::Active);
    assert!(revived.lifecycle.operation.is_none());
    assert!(revived.lifecycle.paused_notice.is_none());
    assert_ne!(revived.runtime.as_ref().unwrap().runtime_id, old_runtime.0);
    assert_eq!(
        revived.runtime.as_ref().unwrap().runtime_version,
        runtime_image()
    );
    assert!(harness
        .credentials
        .material(&revived.id, 1)
        .await
        .unwrap()
        .is_some());
    let requests = harness.backend.requests().await;
    assert!(requests
        .iter()
        .any(|(_, request)| matches!(request, RequestFrame::BootstrapRestore(_))));
    assert!(requests
        .iter()
        .any(|(_, request)| matches!(request, RequestFrame::Restore(_))));
}

#[tokio::test]
async fn worker_rotates_an_image_through_checkpoint_restore_without_installing_again() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let old_runtime = active.runtime.as_ref().unwrap().runtime_id.clone();
    let before = harness
        .backend
        .requests()
        .await
        .iter()
        .filter(|(_, request)| matches!(request, RequestFrame::Install(_)))
        .count();
    let target = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    let identity = SandboxRotationIdentity::capture(&active, &[]).unwrap();
    let expected_revision = active.revision;
    let rotation_at = now + Duration::from_secs(1);
    let mut rotating = active.clone();
    rotating
        .begin_image_rotation(target.clone(), identity, rfc3339_at(rotation_at))
        .unwrap();
    harness
        .store
        .save_sandbox(expected_revision, rotating)
        .await
        .unwrap();

    let mut spec = CodexSandboxSpec::default();
    spec.deployment.image.uri = target.clone();
    let worker = harness.worker_with_adapter(
        "rotation-worker",
        Arc::new(CodexAdapter::with_backend(harness.backend.clone(), spec)),
    );
    for _ in 0..11 {
        assert!(worker.run_one(&active.id, rotation_at).await.unwrap());
    }
    let rotated = harness.stored(&active.id).await;
    assert_eq!(rotated.status, SandboxStatus::Active);
    assert_eq!(rotated.lifecycle.rotation.rotations, 1);
    assert_ne!(rotated.runtime.as_ref().unwrap().runtime_id, old_runtime);
    assert_eq!(rotated.runtime.as_ref().unwrap().runtime_version, target);
    let requests = harness.backend.requests().await;
    assert_eq!(
        requests
            .iter()
            .filter(|(_, request)| matches!(request, RequestFrame::Install(_)))
            .count(),
        before
    );
    assert!(requests
        .iter()
        .any(|(_, request)| matches!(request, RequestFrame::BootstrapRestore(_))));
}

#[tokio::test]
async fn worker_rejects_unpinned_rotation_target_before_bootstrap_restore() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let identity = SandboxRotationIdentity::capture(&active, &[]).unwrap();
    let rotation_at = now + Duration::from_secs(1);
    let mut rotating = active.clone();
    rotating
        .begin_image_rotation("runtime-v3".into(), identity, rfc3339_at(rotation_at))
        .unwrap();
    harness
        .store
        .save_sandbox(active.revision, rotating)
        .await
        .unwrap();
    let worker = harness.worker_with_adapter(
        "rotation-worker",
        Arc::new(CodexAdapter::with_backend(
            harness.backend.clone(),
            CodexSandboxSpec::default(),
        )),
    );
    for _ in 0..6 {
        assert!(worker.run_one(&active.id, rotation_at).await.unwrap());
    }
    let failed = harness.stored(&active.id).await;
    assert_eq!(failed.status, SandboxStatus::Failed);
    let failure = failed.failure.as_ref().unwrap();
    assert_eq!(failure.class, "provisioning_failed");
    assert_eq!(failure.detail, "runtime rejected the lifecycle operation");
    assert!(failed.runtime.is_none());
    assert!(!harness
        .backend
        .requests()
        .await
        .iter()
        .any(|(_, request)| matches!(request, RequestFrame::BootstrapRestore(_))));
}

#[tokio::test]
async fn restarted_worker_reattaches_before_install_with_a_fence_independent_identity() {
    let harness = Harness::new(vec!["touch installed".into()], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    harness.run(&created.id, now).await;
    let mut installing = harness.stored(&created.id).await;
    let revision = installing.revision;
    installing.runtime.as_mut().unwrap().runtime_version =
        CodexSandboxSpec::default().deployment.image.uri;
    installing.revision += 1;
    harness
        .store
        .save_sandbox(revision, installing.clone())
        .await
        .unwrap();
    let expected_runtime = installing.runtime.as_ref().unwrap().runtime_id.clone();

    let restarted: Arc<dyn SandboxAdapter> = Arc::new(CodexAdapter::with_backend(
        harness.backend.clone(),
        CodexSandboxSpec::default(),
    ));
    crate::sandbox::reconciliation::SandboxReconciler::new(
        harness.store.clone(),
        restarted.clone(),
        Arc::new(harness.policy.clone()),
        "test".into(),
    )
    .initialize(now)
    .await
    .unwrap();
    let worker = harness.worker_with_adapter("restarted-worker", restarted);
    assert!(worker.run_one(&created.id, now).await.unwrap());

    let installed = harness.stored(&created.id).await;
    assert_eq!(installed.work.step, SandboxWorkStep::MintInitialCredential);
    assert_eq!(
        installed.runtime.as_ref().unwrap().runtime_id,
        expected_runtime
    );
    let requests = harness.backend.requests().await;
    assert!(matches!(&requests[0].1, RequestFrame::Install(request)
        if request.operation_id == format!("sandbox-install-v1:{}", created.id)
            && request.commands == ["touch installed"]));
}
