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
