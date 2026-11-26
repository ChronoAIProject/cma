//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn workspace_parent_read_and_write_abort_preserve_fresh_transaction_retry() {
    workspace_parent_read_and_write_abort_preserve_fresh_transaction_retry_case("find", false)
        .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn workspace_parent_read_and_write_abort_preserve_fresh_transaction_retry_variant_2() {
    workspace_parent_read_and_write_abort_preserve_fresh_transaction_retry_case("find", true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn workspace_parent_read_and_write_abort_preserve_fresh_transaction_retry_variant_3() {
    workspace_parent_read_and_write_abort_preserve_fresh_transaction_retry_case("update", false)
        .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn workspace_parent_read_and_write_abort_preserve_fresh_transaction_retry_variant_4() {
    workspace_parent_read_and_write_abort_preserve_fresh_transaction_retry_case("update", true)
        .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn nested_child_claim_retries_aborted_original_parent_authority() {
    nested_child_claim_retries_aborted_original_parent_authority_case(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn nested_child_claim_retries_aborted_original_parent_authority_variant_2() {
    nested_child_claim_retries_aborted_original_parent_authority_case(true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn workspace_observations_follow_terminal_event_rollback_and_commit_reconciliation() {
    use layer2_core::sandbox::WorkspacePrepareObservation;
    use layer3_adapter::workspace_prepare_observation::{Code, Observation, Stage};
    use mongodb::event::command::CommandEvent;

    let f = Fixture::new().await;
    let parent = sandbox("sbx_workspace_observation");
    f.independent.create_sandbox(parent.clone()).await.unwrap();
    let mut original = workspace(&parent);
    original.work.execution = Some(Default::default());
    let first = WorkspacePrepareObservation::new(
        Observation {
            stage: Stage::PrepareRpc,
            code: Code::RepositorySourceUnavailable,
            wire_retryable: Some(true),
        },
        0,
        None,
    );
    original.record_prepare_observation(first.clone(), false);
    original.work.attempts = 1;
    f.independent
        .create_workspace(original.clone())
        .await
        .unwrap();

    let claimed = f
        .independent
        .claim_workspace_work(&original.id, "worker", now(), work_lease_deadline())
        .await
        .unwrap()
        .unwrap();
    let mut failed = claimed.clone();
    let terminal = WorkspacePrepareObservation::new(
        Observation {
            stage: Stage::PrepareRpc,
            code: Code::WorkspacePrepareOutcomeUnknown,
            wire_retryable: Some(false),
        },
        1,
        None,
    );
    failed.record_prepare_observation(terminal.clone(), true);
    failed.work.schedule(WorkspaceWorkStep::None, MINUTE.into());
    let failure = TypedFailure {
        class: "workspace_runtime_rejected".into(),
        detail: "workspace runtime rejected the operation".into(),
    };
    failed
        .transition(
            WorkspaceStatus::Failed,
            MINUTE.into(),
            Some(failure.clone()),
        )
        .unwrap();
    let event = SandboxEventKind::WorkspaceFailed {
        workspace_id: claimed.id.clone(),
        failure,
    };
    let transaction = SandboxTransaction::SaveWorkspaceAndAppend {
        expected_revision: claimed.revision,
        workspace: Box::new(failed.clone()),
        event_at: MINUTE.into(),
        event: event.clone(),
        repository_runtime: None,
    };
    let conflicting = layer2_core::sandbox::SandboxEvent {
        sandbox_id: parent.id.clone(),
        seq: 1,
        appended_at: MINUTE.into(),
        kind: SandboxEventKind::Created,
    };
    let conflict = f
        .independent
        .sandbox_events
        .insert_one(crate::store::sandbox_events::event_document(&conflicting).unwrap())
        .await
        .unwrap();
    let publish = |transaction, ambiguous| {
        let authority = WorkAuthority::new(
            ClaimedWork::Workspace(Box::new(claimed.clone())),
            Instant::now(),
        )
        .with_parent(parent.clone())
        .unwrap();
        let f = &f;
        let claimed = &claimed;
        async move {
            authority
                .run(
                    async {
                        if ambiguous {
                            f.fail("commitTransaction", 5, true).await;
                        }
                        crate::store::work_publication::scope(&f.store, f.store.commit(transaction))
                            .await
                    },
                    || async {
                        f.independent
                            .renew_workspace_authority(claimed, &now(), &work_lease_deadline())
                            .await
                    },
                )
                .await
        }
    };
    let rolled_back = publish(transaction.clone(), false).await;
    let after_rollback = f
        .independent
        .get_workspace(&claimed.id)
        .await
        .unwrap()
        .unwrap();
    let rollback_events = f
        .independent
        .sandbox_events(&parent.id, 0, 10)
        .await
        .unwrap();
    f.independent
        .sandbox_events
        .delete_one(doc! {"_id":conflict.inserted_id})
        .await
        .unwrap();

    let committed = publish(transaction, true).await;
    f.clear().await;
    let commits = f.events.lock().unwrap().iter().filter(|event| {
        matches!(event, CommandEvent::Started(started) if started.command_name == "commitTransaction")
    }).count();
    let stored = f.independent.get_workspace(&claimed.id).await.unwrap();
    let events = f
        .independent
        .sandbox_events(&parent.id, 0, 10)
        .await
        .unwrap();
    f.drop_database().await;

    assert_eq!(rolled_back, Err(SandboxStoreError::SequenceConflict));
    assert_eq!(after_rollback.status, WorkspaceStatus::Preparing);
    assert_eq!(after_rollback.revision, claimed.revision);
    assert_eq!(
        after_rollback.work.prepare_observations,
        original.work.prepare_observations
    );
    assert!(rollback_events.is_empty());
    committed.unwrap();
    assert_eq!(commits, 5);
    assert_eq!(stored, Some(failed.clone()));
    let linked = failed.work.prepare_observations.unwrap();
    let request = original.original_prepare_request().unwrap();
    assert_eq!(linked.operation_id, request.operation_id);
    assert_eq!(linked.request_sha256, request.request_sha256);
    assert_eq!(linked.first, first);
    assert_eq!(linked.terminal, Some(terminal));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, event);
}
