use super::*;
use layer2_core::sandbox::ProvisioningExecution;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn execution_pins_survive_reopen_and_reject_cas_or_transaction_replacement() {
    let (client, name) = database("execution_pins").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_execution");
    store.create_sandbox(parent.clone()).await.unwrap();
    let child = ready_workspace(&parent, "wks_execution", "execution");
    store.create_workspace(child.clone()).await.unwrap();
    let policy = ProvisioningExecution {
        command_timeout_seconds: 20,
        total_timeout_seconds: 30,
        clone_depth: 2,
    };
    let mut pinned = parent.clone();
    pinned.work.execution = Some(policy);
    pinned.revision += 1;
    store
        .save_sandbox(parent.revision, pinned.clone())
        .await
        .unwrap();
    let mut pinned_child = child.clone();
    pinned_child.work.execution = Some(policy);
    pinned_child.revision += 1;
    store
        .save_workspace(child.revision, pinned_child.clone())
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.get_sandbox(&parent.id).await.unwrap(),
        Some(pinned.clone())
    );
    assert_eq!(
        reopened.get_workspace(&child.id).await.unwrap(),
        Some(pinned_child.clone())
    );
    for replacement in [None, Some(ProvisioningExecution::default())] {
        let mut next = pinned.clone();
        next.work.execution = replacement;
        next.revision += 1;
        assert_eq!(
            reopened.save_sandbox(pinned.revision, next.clone()).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(
            reopened
                .commit(SandboxTransaction::SaveSandboxAndAppend {
                    command_revival: None,
                    released_runtime: None,
                    expected_revision: pinned.revision,
                    sandbox: Box::new(next),
                    event_at: NOW.into(),
                    event: SandboxEventKind::WorkStepCompleted {
                        step: SandboxWorkStep::RunInstall
                    }
                })
                .await,
            Err(SandboxStoreError::Conflict)
        );
        let mut next = pinned_child.clone();
        next.work.execution = replacement;
        next.revision += 1;
        assert_eq!(
            reopened
                .save_workspace(pinned_child.revision, next.clone())
                .await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(
            reopened
                .commit(SandboxTransaction::SaveWorkspaceAndAppend {
                    repository_runtime: None,
                    expected_revision: pinned_child.revision,
                    workspace: Box::new(next),
                    event_at: NOW.into(),
                    event: SandboxEventKind::WorkspaceReady {
                        workspace_id: child.id.clone(),
                        resolved_commit: pinned_child.resolved_commit.clone().unwrap()
                    }
                })
                .await,
            Err(SandboxStoreError::Conflict)
        );
    }
    assert_eq!(
        reopened.get_sandbox(&parent.id).await.unwrap(),
        Some(pinned)
    );
    assert_eq!(
        reopened.get_workspace(&child.id).await.unwrap(),
        Some(pinned_child)
    );
    client.database(&name).drop().await.unwrap();
}
