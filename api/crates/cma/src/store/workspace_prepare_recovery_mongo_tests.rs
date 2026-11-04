use super::*;
use layer2_core::sandbox::store::WorkspaceAdmissionStore;
use layer2_core::{Authorization, Caller, Capability};

fn failed(parent: &Sandbox) -> Workspace {
    let mut value = workspace(parent);
    value.status = WorkspaceStatus::Failed;
    value.ready_at = None;
    value.resolved_commit = None;
    value.failed = Some(TypedFailure {
        class: "prepare_failed".into(),
        detail: "terminal".into(),
    });
    value.profile.digest = layer2_core::profiles::content_digest(&value.profile.content).unwrap();
    value.work.execution = Some(Default::default());
    value
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_prepare_recovery_atomic_replay_auth_and_original_data_retention() {
    let (client, name, store) = database("prepare_recovery").await;
    let parent = sandbox(
        "sbx_recover",
        "owner",
        "recovery",
        SandboxStatus::Active,
        "runtime-v2",
    );
    store.create_sandbox(parent.clone()).await.unwrap();
    let original = failed(&parent);
    store.create_workspace(original.clone()).await.unwrap();
    let owner = Caller::new(
        SubjectId("owner".into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let foreign = Caller::new(
        SubjectId("foreign".into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    assert!(matches!(
        store
            .begin_workspace_prepare_recovery(&original.id, &foreign, "foreign", NOW.into())
            .await,
        Err(SandboxStoreError::NotFound)
    ));
    let (one, two) = tokio::join!(
        store.begin_workspace_prepare_recovery(&original.id, &owner, "first", NOW.into()),
        store.begin_workspace_prepare_recovery(&original.id, &owner, "first", NOW.into())
    );
    let recovered = one.unwrap();
    assert_eq!(recovered, two.unwrap());
    assert_eq!(recovered.profile, original.profile);
    assert_eq!(recovered.path, original.path);
    assert_eq!(recovered.source, original.source);
    assert_eq!(recovered.id, original.id);
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap().unwrap(),
        parent
    );
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
    assert!(matches!(
        store
            .begin_workspace_prepare_recovery(&original.id, &owner, "new", NOW.into())
            .await,
        Err(SandboxStoreError::WorkspaceRecoveryConflict)
    ));
    let restarted = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        restarted
            .begin_workspace_prepare_recovery(&original.id, &owner, "first", NOW.into())
            .await
            .unwrap(),
        recovered
    );
    client.database(&name).drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_prepare_recovery_refuses_any_historical_agent_and_preserves_failed_workspace() {
    let (client, name, store) = database("prepare_recovery_children").await;
    let parent = sandbox(
        "sbx_children",
        "owner",
        "recovery",
        SandboxStatus::Active,
        "runtime-v2",
    );
    store.create_sandbox(parent.clone()).await.unwrap();
    let original = failed(&parent);
    store.create_workspace(original.clone()).await.unwrap();
    store
        .create_agent(agent(&parent, "agt_deleted", AgentStatus::Deleted))
        .await
        .unwrap();
    let admin = Caller::new(
        SubjectId("admin".into()),
        Authorization::new([Capability::ControlAnySandbox]),
    );
    assert!(matches!(
        store
            .begin_workspace_prepare_recovery(&original.id, &admin, "first", NOW.into())
            .await,
        Err(SandboxStoreError::WorkspaceRecoveryConflict)
    ));
    assert_eq!(
        store.get_workspace(&original.id).await.unwrap().unwrap(),
        original
    );
    client.database(&name).drop().await.unwrap();
}
