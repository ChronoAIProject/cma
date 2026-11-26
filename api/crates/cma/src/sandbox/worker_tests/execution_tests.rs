use super::*;
use crate::sandbox::workspace_worker::{
    WorkspaceWorker, WorkspaceWorkerConfig, WorkspaceWorkerDependencies,
};
use layer2_core::sandbox::store::{SandboxTransaction, SandboxTransactionStore, WorkspaceStore};
use layer2_core::sandbox::{
    CreateWorkspace, ProvisioningExecution, WorkspaceService, WorkspaceWorkStep,
};

fn custom_execution() -> ProvisioningExecution {
    ProvisioningExecution {
        command_timeout_seconds: 20,
        total_timeout_seconds: 30,
        clone_depth: 2,
    }
}

#[tokio::test]
async fn pending_install_replays_the_persisted_policy_after_deployment_changes() {
    let harness = Harness::new(vec!["true".into()], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let original = harness.worker_with_execution(
        "original",
        harness.credentials.clone(),
        Duration::from_secs(60),
        Arc::new(harness.policy.clone()),
        custom_execution(),
    );
    original.run_one(&created.id, now).await.unwrap();
    assert_eq!(
        harness.stored(&created.id).await.work.execution,
        Some(custom_execution())
    );
    harness.backend.lose_next_response(WireOperation::Install);
    original.run_one(&created.id, now).await.unwrap();
    harness
        .worker
        .run_one(&created.id, now + Duration::from_secs(6))
        .await
        .unwrap();
    let requests: Vec<_> = harness
        .backend
        .requests()
        .await
        .into_iter()
        .filter_map(|(_, frame)| match frame {
            RequestFrame::Install(request) => Some(request),
            _ => None,
        })
        .collect();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    assert_eq!(requests[1].timeout_seconds, 20);
    assert_eq!(requests[1].total_timeout_seconds, 30);
    let current = harness.stored(&created.id).await;
    assert_eq!(current.work.step, SandboxWorkStep::MintInitialCredential);
    let mut changed = current.clone();
    changed.revision += 1;
    changed.work.execution = None;
    assert_eq!(
        harness
            .store
            .save_sandbox(current.revision, changed.clone())
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        harness
            .store
            .commit(SandboxTransaction::SaveSandboxAndAppend {
                command_revival: None,
                released_runtime: None,
                expected_revision: current.revision,
                sandbox: Box::new(changed),
                event_at: rfc3339_at(now),
                event: SandboxEventKind::WorkStepCompleted {
                    step: SandboxWorkStep::RunInstall
                }
            })
            .await,
        Err(SandboxStoreError::Conflict)
    );
}

#[tokio::test]
async fn workspace_clone_and_setup_replay_the_first_persisted_policy() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    harness.drive_active(&created.id, now).await;
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = seed_workspace_profile(&profiles, 0).await;
    let service = WorkspaceService::new(harness.store.clone(), profiles);
    let workspace = service
        .create(
            &caller("owner", [Capability::UseOwnSandboxes]),
            &created.id,
            CreateWorkspace {
                profile: reference,
                confirmed_digest: None,
            },
        )
        .await
        .unwrap();
    let worker = |execution| {
        WorkspaceWorker::new(
            WorkspaceWorkerDependencies {
                store: harness.store.clone(),
                adapter: harness.adapter.clone(),
            },
            WorkspaceWorkerConfig::new("workspace", "test").with_execution(execution),
        )
    };
    let original = worker(custom_execution());
    harness
        .backend
        .lose_next_response(WireOperation::PrepareWorkspace);
    original.run_one(&workspace.id, now).await.unwrap();
    let replacement = worker(ProvisioningExecution::default());
    replacement
        .run_one(&workspace.id, now + Duration::from_secs(31))
        .await
        .unwrap();
    replacement
        .run_one(&workspace.id, now + Duration::from_secs(31))
        .await
        .unwrap();
    let mut prepares = Vec::new();
    let mut setups = Vec::new();
    for (_, request) in harness.backend.requests().await {
        match request {
            RequestFrame::PrepareWorkspace(request) => prepares.push(request),
            RequestFrame::WorkspaceSetup(request) => setups.push(request),
            _ => (),
        }
    }
    assert_eq!(prepares.len(), 2);
    assert_eq!(prepares[0], prepares[1]);
    assert_eq!(prepares[1].clone_depth, 2);
    assert_eq!(setups[0].timeout_seconds, 20);
    assert_eq!(setups[0].total_timeout_seconds, 30);
    let current = harness
        .store
        .get_workspace(&workspace.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.work.step, WorkspaceWorkStep::None);
    let mut changed = current.clone();
    changed.revision += 1;
    changed.work.execution = Some(ProvisioningExecution::default());
    assert_eq!(
        harness
            .store
            .save_workspace(current.revision, changed.clone())
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        harness
            .store
            .commit(SandboxTransaction::SaveWorkspaceAndAppend {
                repository_runtime: None,
                expected_revision: current.revision,
                workspace: Box::new(changed),
                event_at: rfc3339_at(now),
                event: SandboxEventKind::WorkspaceRemoved {
                    workspace_id: workspace.id
                }
            })
            .await,
        Err(SandboxStoreError::Conflict)
    );
}

#[test]
fn execution_policy_is_bounded_and_cannot_be_replaced_or_erased() {
    let policy = custom_execution();
    assert!(policy.validate().is_ok());
    assert!(ProvisioningExecution::validate_successor(None, Some(policy)).is_ok());
    assert!(ProvisioningExecution::validate_successor(Some(policy), Some(policy)).is_ok());
    assert!(ProvisioningExecution::validate_successor(Some(policy), None).is_err());
    for invalid in [
        ProvisioningExecution {
            command_timeout_seconds: 0,
            ..policy
        },
        ProvisioningExecution {
            total_timeout_seconds: 1,
            ..policy
        },
        ProvisioningExecution {
            clone_depth: 0,
            ..policy
        },
    ] {
        assert!(ProvisioningExecution::validate_successor(None, Some(invalid)).is_err());
    }
}
