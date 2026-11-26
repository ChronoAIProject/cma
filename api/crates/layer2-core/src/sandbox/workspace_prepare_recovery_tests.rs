use super::*;

async fn fixture() -> (
    Arc<InMemorySandboxStore>,
    Arc<WorkspaceService>,
    Workspace,
    Sandbox,
) {
    let store = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    let mut parent = sandbox("sbx_recovery", SandboxStatus::Active, vec![], 4);
    parent
        .work
        .schedule(SandboxWorkStep::WaitForCredentialRotation, NOW.into());
    store.create_sandbox(parent.clone()).await.unwrap();
    let service = WorkspaceService::new(store.clone(), profiles);
    let mut workspace = service
        .create_inline(
            &caller("consumer"),
            &parent.id,
            CreateInlineWorkspace {
                source: InlineWorkspaceSource::Empty,
                name: Some("original".into()),
            },
        )
        .await
        .unwrap();
    let revision = workspace.revision;
    workspace.work.execution = Some(ProvisioningExecution::default());
    workspace.work.schedule(WorkspaceWorkStep::None, NOW.into());
    workspace
        .transition(
            WorkspaceStatus::Failed,
            NOW.into(),
            Some(TypedFailure {
                class: "prepare_failed".into(),
                detail: "original failure".into(),
            }),
        )
        .unwrap();
    store
        .save_workspace(revision, workspace.clone())
        .await
        .unwrap();
    let parent = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    (store, service, workspace, parent)
}

#[tokio::test]
async fn prepare_recovery_preserves_original_identity_and_replays_concurrent_admission() {
    let (store, service, original, parent) = fixture().await;
    let owner = caller("consumer");
    let (one, two) = tokio::join!(
        service.recover_prepare(&owner, &original.id, "first"),
        service.recover_prepare(&owner, &original.id, "first")
    );
    let recovered = one.unwrap();
    assert_eq!(recovered, two.unwrap());
    assert_eq!(recovered.id, original.id);
    assert_eq!(recovered.profile, original.profile);
    assert_eq!(recovered.source, original.source);
    assert_eq!(recovered.path, original.path);
    assert_eq!(recovered.created_at, original.created_at);
    assert_eq!(recovered.origin, original.origin);
    assert_eq!(recovered.work.step, WorkspaceWorkStep::RecoverPrepare);
    assert_eq!(recovered.status, WorkspaceStatus::Preparing);
    assert!(recovered.ready_at.is_none());
    assert!(recovered.resolved_commit.is_none());
    assert_eq!(
        recovered
            .work
            .prepare_recovery
            .as_ref()
            .unwrap()
            .attempts
            .len(),
        1
    );
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap().unwrap(),
        parent
    );
    assert_eq!(
        service
            .recover_prepare(&owner, &original.id, "second")
            .await
            .unwrap_err(),
        WorkspaceServiceError::RecoveryConflict
    );
}

#[tokio::test]
async fn prepare_recovery_preserves_repository_snapshot_request_and_refuses_changed_source() {
    let (store, service, mut original, _) = fixture().await;
    let revision = original.revision;
    let url = "https://github.com/example/original".to_string();
    let reference = Some("release".to_string());
    original.profile.content.location = crate::profiles::WorkspaceLocation::RepositorySnapshot {
        connection_id: None,
        url: url.clone(),
        reference: reference.clone(),
    };
    original.profile.digest = crate::profiles::content_digest(&original.profile.content).unwrap();
    original.source = WorkspaceSource::RepositorySnapshot {
        url: url.clone(),
        reference: reference.clone(),
        source_commit: None,
    };
    original.revision += 1;
    store
        .save_workspace(revision, original.clone())
        .await
        .unwrap();
    let recovered = service
        .recover_prepare(&caller("consumer"), &original.id, "snapshot")
        .await
        .unwrap();
    let request = &recovered.work.prepare_recovery.as_ref().unwrap().original;
    assert_eq!(
        request.source,
        layer3_adapter::WorkspaceSource::RepositorySnapshot { url, reference }
    );
    assert!(request.has_valid_sha256());
    assert_eq!(request, &original.original_prepare_request().unwrap());
    assert_eq!(recovered.source, original.source);
    assert_eq!(
        service
            .recover_prepare(&caller("consumer"), &original.id, "snapshot")
            .await
            .unwrap(),
        recovered
    );
    for pinned in [false, true] {
        let mut changed = recovered.clone();
        let WorkspaceSource::RepositorySnapshot {
            reference,
            source_commit,
            ..
        } = &mut changed.source
        else {
            unreachable!()
        };
        if pinned {
            *source_commit = Some("a".repeat(40));
        } else {
            *reference = Some("different".into());
        }
        assert_eq!(
            changed.validate_prepare_recovery_binding(),
            Err(SandboxStoreError::WorkspaceRecoveryConflict)
        );
    }
    let mut completed = recovered.clone();
    let WorkspaceSource::RepositorySnapshot { source_commit, .. } = &mut completed.source else {
        unreachable!()
    };
    *source_commit = Some("b".repeat(40));
    completed.record_resolved_commit("c".repeat(40)).unwrap();
    completed
        .work
        .schedule(WorkspaceWorkStep::Setup, NOW.into());
    let receipt_request =
        layer3_adapter::WorkspacePrepareRecoveryRequest::new("snapshot", request.clone(), None);
    completed.work.prepare_recovery.as_mut().unwrap().attempts[0].completion =
        Some(WorkspacePrepareRecoveryCompletion {
            prepared: layer3_adapter::WorkspaceReceipt {
                operation_id: receipt_request.operation_id.clone(),
                request_sha256: receipt_request.request_sha256.clone(),
                workspace_id: original.id.to_string(),
                name: original.name.clone(),
                path: original.path.clone(),
                resolved_commit: "c".repeat(40),
                source_commit: Some("b".repeat(40)),
            },
            preserved_path: receipt_request.preserved_path(),
            original_journal_sha256: "d".repeat(64),
        });
    completed.validate_prepare_recovery_binding().unwrap();
    let completion_revision = completed.revision;
    completed.revision += 1;
    store
        .save_workspace(completion_revision, completed.clone())
        .await
        .unwrap();
    assert_eq!(
        service
            .recover_prepare(&caller("consumer"), &original.id, "snapshot")
            .await
            .unwrap(),
        completed
    );
    let WorkspaceSource::RepositorySnapshot { source_commit, .. } = &mut completed.source else {
        unreachable!()
    };
    *source_commit = Some("e".repeat(40));
    assert_eq!(
        completed.validate_prepare_recovery_binding(),
        Err(SandboxStoreError::WorkspaceRecoveryConflict)
    );
}

#[tokio::test]
async fn prepare_recovery_admin_is_audited_and_foreign_or_unprivileged_cannot_admit() {
    let (store, service, original, _) = fixture().await;
    assert_eq!(
        service
            .recover_prepare(&caller("foreign"), &original.id, "foreign")
            .await
            .unwrap_err(),
        WorkspaceServiceError::NotFound
    );
    let denied = Caller::new(SubjectId("consumer".into()), Authorization::new([]));
    assert_eq!(
        service
            .recover_prepare(&denied, &original.id, "denied")
            .await
            .unwrap_err(),
        WorkspaceServiceError::Forbidden
    );
    assert_eq!(
        store.get_workspace(&original.id).await.unwrap().unwrap(),
        original
    );
    let admin = Caller::new(
        SubjectId("operator".into()),
        Authorization::new([Capability::ControlAnySandbox]),
    );
    let recovered = service
        .recover_prepare(&admin, &original.id, "admin")
        .await
        .unwrap();
    assert_eq!(recovered.owner, original.owner);
    assert_eq!(
        recovered.work.prepare_recovery.unwrap().attempts[0].requested_by,
        admin.subject
    );
}

#[tokio::test]
async fn prepare_recovery_new_key_after_terminal_failure_retains_attempts_and_old_key_never_reschedules(
) {
    let (store, service, original, _) = fixture().await;
    let owner = caller("consumer");
    let mut failed = service
        .recover_prepare(&owner, &original.id, "first")
        .await
        .unwrap();
    let revision = failed.revision;
    let failure = TypedFailure {
        class: "retry_failed".into(),
        detail: "terminal".into(),
    };
    failed.work.prepare_recovery.as_mut().unwrap().attempts[0].failed = Some(failure.clone());
    failed.work.schedule(WorkspaceWorkStep::None, NOW.into());
    failed
        .transition(WorkspaceStatus::Failed, NOW.into(), Some(failure))
        .unwrap();
    store
        .save_workspace(revision, failed.clone())
        .await
        .unwrap();
    assert_eq!(
        service
            .recover_prepare(&owner, &original.id, "first")
            .await
            .unwrap(),
        failed
    );
    let second = service
        .recover_prepare(&owner, &original.id, "second")
        .await
        .unwrap();
    let attempts = &second.work.prepare_recovery.as_ref().unwrap().attempts;
    assert_eq!(attempts.len(), 2);
    assert!(attempts[0].failed.is_some());
    assert!(attempts[1].failed.is_none());
    assert_eq!(attempts[1].original_failure, failed.failed.unwrap());
    assert_eq!(
        service
            .recover_prepare(&owner, &original.id, "first")
            .await
            .unwrap(),
        second
    );
}

#[tokio::test]
async fn prepare_recovery_refuses_used_changed_or_pending_authority_without_mutation() {
    let (_, _, original, parent) = fixture().await;
    let owner = caller("consumer");
    let changes: [fn(&mut Workspace); 7] = [
        |w| w.ready_at = Some(NOW.into()),
        |w| w.resolved_commit = Some("a".repeat(40)),
        |w| w.next_worktree_number = 2,
        |w| w.removed_at = Some(NOW.into()),
        |w| w.work.step = WorkspaceWorkStep::Prepare,
        |w| w.profile.digest = "changed".into(),
        |w| w.work.execution = None,
    ];
    for change in changes {
        let mut workspace = original.clone();
        change(&mut workspace);
        let before = workspace.clone();
        assert!(workspace
            .admit_prepare_recovery(&parent, &owner, "first", false, false, NOW.into())
            .is_err());
        assert_eq!(workspace, before);
    }
    for (agents, worktrees) in [(true, false), (false, true)] {
        let mut workspace = original.clone();
        assert!(workspace
            .admit_prepare_recovery(&parent, &owner, "first", agents, worktrees, NOW.into())
            .is_err());
        assert_eq!(workspace, original);
    }
    let mut pending = parent.clone();
    pending.work.step = SandboxWorkStep::RunLifecycleOperation;
    let mut workspace = original.clone();
    assert!(workspace
        .admit_prepare_recovery(&pending, &owner, "first", false, false, NOW.into())
        .is_err());
    let mut admitted = original.clone();
    admitted
        .admit_prepare_recovery(&parent, &owner, "first", false, false, NOW.into())
        .unwrap();
    admitted.source = WorkspaceSource::GitRepository {
        url: "https://example.org/other.git".into(),
        reference: None,
    };
    assert!(admitted
        .admit_prepare_recovery(&parent, &owner, "first", false, false, NOW.into())
        .is_err());
}
