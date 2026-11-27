use super::*;

#[test]
fn workspace_admission_requires_a_canonical_path_and_initial_revision() {
    let parent = sandbox_in(S::Active);
    let valid = workspace();
    assert_eq!(valid.validate_prerequisites(&parent), Ok(()));

    for refused in [
        Workspace {
            path: "/home/cma/other".into(),
            ..valid.clone()
        },
        Workspace {
            revision: 0,
            ..valid
        },
    ] {
        assert_eq!(
            refused.validate_prerequisites(&parent),
            Err(SandboxModelError::invalid_value("workspace.path"))
        );
    }
}

#[test]
fn worktree_admission_requires_the_path_of_its_own_checkout() {
    let parent = workspace();
    let mut tree = worktree();
    assert_eq!(tree.validate_prerequisites(&parent), Ok(()));

    tree.path = "/home/cma/project/.worktrees/wt-2".into();
    assert_eq!(
        tree.validate_prerequisites(&parent),
        Err(SandboxModelError::invalid_value("worktree"))
    );
}

#[tokio::test]
async fn workspace_store_refuses_duplicate_names_and_execution_rewrites() {
    let store = InMemorySandboxStore::default();
    let mut original = workspace();
    original.work.execution = Some(ProvisioningExecution::default());
    store.create_workspace(original.clone()).await.unwrap();

    let duplicate = Workspace {
        id: WorkspaceId("wks_duplicate".into()),
        ..original.clone()
    };
    assert_eq!(
        store.create_workspace(duplicate.clone()).await,
        Err(SandboxStoreError::Duplicate)
    );
    assert_eq!(store.get_workspace(&duplicate.id).await.unwrap(), None);

    let mut rewritten = original.clone();
    rewritten.revision += 1;
    rewritten.work.execution.as_mut().unwrap().clone_depth += 1;
    assert_eq!(
        store.save_workspace(original.revision, rewritten).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_workspace(&original.id).await.unwrap(),
        Some(original)
    );
}

#[tokio::test]
async fn workspace_renewal_refuses_a_retained_row_with_mismatched_parent_ownership() {
    const EXPIRES: &str = "2026-09-05T00:03:00Z";
    const EXTENDED: &str = "2026-09-05T00:04:00Z";

    let store = InMemorySandboxStore::default();
    let parent = sandbox_in(S::Active);
    store.create_sandbox(parent.clone()).await.unwrap();

    // Model a retained row with a current lease and inconsistent lineage.
    let mut retained = workspace();
    retained.owner = SubjectId::new("unrelated-owner");
    retained.status = W::Preparing;
    retained.ready_at = None;
    retained.resolved_commit = None;
    retained
        .work
        .schedule(WorkspaceWorkStep::Prepare, NOW.into());
    retained
        .work
        .claim("worker".into(), NOW, EXPIRES.into())
        .unwrap();
    store.create_workspace(retained.clone()).await.unwrap();

    assert_eq!(
        store
            .renew_workspace_authority(&retained, TRANSITION_AT, EXTENDED)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        store.get_workspace(&retained.id).await.unwrap(),
        Some(retained)
    );
    assert!(store
        .sandbox_events(&parent.id, 0, 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
}
