use super::*;
use crate::store::sandbox_mongo_integration_tests::sandbox;
use layer2_core::profiles::{WorkspaceLocation, WorkspaceProfileContent, WorkspaceProfileSnapshot};
use layer2_core::sandbox::store::{
    SandboxTransaction, SandboxTransactionStore, WorkspaceStore, WorkspaceWorkerStore,
};
use layer2_core::sandbox::{
    TypedFailure, Workspace, WorkspaceOrigin, WorkspaceSource, WorkspaceStatus, WorkspaceWorkState,
    WorkspaceWorkStep,
};
use layer2_core::{WorkspaceId, WorkspaceProfileId};

fn workspace(parent: &Sandbox) -> Workspace {
    Workspace {
        id: WorkspaceId("wks_fence".into()),
        sandbox_id: parent.id.clone(),
        owner: parent.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("wkp_fence".into()),
            revision: 1,
            digest: "d".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "fence".into(),
                },
                name: "fence".into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: "fence".into(),
        path: "/home/cma/fence".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: None,
        status: WorkspaceStatus::Preparing,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, MINUTE.into()),
        next_worktree_number: 1,
        created_at: MINUTE.into(),
        ready_at: None,
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn workspace_initial_renewal_survives_a_known_transaction_abort() {
    workspace_renewal_after_abort(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn workspace_initial_renewal_stops_before_effect_after_parent_replacement() {
    workspace_renewal_after_abort(true).await;
}

async fn workspace_renewal_after_abort(replace_parent: bool) {
    let f = Fixture::new().await;
    let parent = sandbox("sbx_workspace_renewal");
    f.independent.create_sandbox(parent.clone()).await.unwrap();
    let child = workspace(&parent);
    f.independent.create_workspace(child.clone()).await.unwrap();
    let claimed = f
        .independent
        .claim_workspace_work(&child.id, "worker", MINUTE.into(), work_lease_deadline())
        .await
        .unwrap()
        .unwrap();
    let authority = WorkAuthority::new(
        ClaimedWork::Workspace(Box::new(claimed.clone())),
        Instant::now(),
    )
    .with_parent(parent.clone())
    .unwrap();

    let store = f.store.clone().with_workspace_authority_retry(true);
    if replace_parent {
        f.independent
            .sandboxes
            .update_one(
                doc! {"_id":parent.id.as_str()},
                doc! {"$set":{"owner":"replacement","record.owner":"replacement"},
                "$inc":{"revision":1_i64,"record.revision":1_i64}},
            )
            .await
            .unwrap();
    }
    f.fail("find", 1, false).await;
    let mut executed = false;
    let result = crate::store::work_publication::scope(
        &store,
        authority.run(
            async {
                executed = true;
                Ok(())
            },
            || async {
                store
                    .renew_workspace_authority(&claimed, &now(), &work_lease_deadline())
                    .await
            },
        ),
    )
    .await;
    f.clear().await;
    let starts = f.starts();
    let stored = f.independent.get_workspace(&child.id).await.unwrap();
    f.drop_database().await;

    assert_fresh_transactions(&starts, 2);
    let stored = stored.unwrap();
    if replace_parent {
        assert_eq!(result, Err(SandboxStoreError::Conflict));
        assert!(!executed);
        assert_eq!(stored, claimed);
        return;
    }
    result.expect("one known abort must not consume the original Workspace claim");
    assert!(executed);
    assert_eq!(stored.work.step, claimed.work.step);
    assert_eq!(stored.revision, claimed.revision);
    let lease = claimed.work.lease.unwrap();
    assert!(stored.work.owns(&lease.holder, lease.fence));
}

#[derive(Clone, Copy)]
enum PinFault {
    KnownAbort,
    UnknownApplied,
    UnknownUnapplied,
}

async fn workspace_pin_fault(fault: PinFault) {
    let f = Fixture::new().await;
    let parent = sandbox("sbx_pin_fault");
    f.independent.create_sandbox(parent.clone()).await.unwrap();
    let child = workspace(&parent);
    f.independent.create_workspace(child.clone()).await.unwrap();
    let claimed = f
        .independent
        .claim_workspace_work(&child.id, "worker", MINUTE.into(), work_lease_deadline())
        .await
        .unwrap()
        .unwrap();
    let authority = WorkAuthority::new(
        ClaimedWork::Workspace(Box::new(claimed.clone())),
        Instant::now(),
    )
    .with_parent(parent)
    .unwrap();
    let store = f.store.clone().with_workspace_authority_retry(true);
    let mut pinned = claimed.clone();
    pinned.work.execution = Some(layer2_core::sandbox::ProvisioningExecution::default());
    pinned.revision += 1;
    let mut effects = 0;

    let result = crate::store::work_publication::scope(
        &store,
        authority.run(
            async {
                match fault {
                    PinFault::KnownAbort => f.fail("update", 1, false).await,
                    PinFault::UnknownApplied => f.fail("commitTransaction", 5, true).await,
                    PinFault::UnknownUnapplied => {
                        f.fail_command(5, doc! {
                            "failCommands": ["commitTransaction"],
                            "errorCode": 112_i32,
                            "errorLabels": ["UnknownTransactionCommitResult", "TransientTransactionError"],
                        })
                        .await;
                    }
                }
                store.save_workspace(claimed.revision, pinned.clone()).await?;
                effects += 1;
                Ok(())
            },
            || async {
                f.independent
                    .renew_workspace_authority(&claimed, &now(), &work_lease_deadline())
                    .await
            },
        ),
    )
    .await;
    f.clear().await;
    let starts = f.starts();
    let stored = f
        .independent
        .get_workspace(&child.id)
        .await
        .unwrap()
        .unwrap();
    f.drop_database().await;

    match fault {
        PinFault::KnownAbort => {
            result.expect("known-aborted pin can retry under the original claim");
            assert_fresh_transactions(&starts, 2);
            assert_eq!(effects, 1);
            assert!(layer2_core::sandbox::store::workspace_content_matches(
                Some(&stored),
                &pinned
            ));
        }
        PinFault::UnknownApplied => {
            assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
            assert_fresh_transactions(&starts, 1);
            assert_eq!(effects, 0);
            assert!(layer2_core::sandbox::store::workspace_content_matches(
                Some(&stored),
                &pinned
            ));
        }
        PinFault::UnknownUnapplied => {
            assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
            assert_fresh_transactions(&starts, 1);
            assert_eq!(effects, 0);
            assert!(layer2_core::sandbox::store::workspace_content_matches(
                Some(&stored),
                &claimed
            ));
        }
    }
}

async fn workspace_parent_read_and_write_abort_preserve_fresh_transaction_retry_case(
    command: &'static str,
    scoped: bool,
) {
    let f = Fixture::new().await;
    let parent = sandbox("sbx_workspace_fence");
    f.independent.create_sandbox(parent.clone()).await.unwrap();
    let original = workspace(&parent);
    f.independent
        .create_workspace(original.clone())
        .await
        .unwrap();
    let claimed = f
        .independent
        .claim_workspace_work(&original.id, "worker", MINUTE.into(), work_lease_deadline())
        .await
        .unwrap()
        .unwrap();
    let failure = TypedFailure {
        class: "setup_failed".into(),
        detail: "fixture publication".into(),
    };
    let mut failed = claimed.clone();
    failed.work.schedule(WorkspaceWorkStep::None, MINUTE.into());
    failed
        .transition(
            WorkspaceStatus::Failed,
            MINUTE.into(),
            Some(failure.clone()),
        )
        .unwrap();
    let transaction = SandboxTransaction::SaveWorkspaceAndAppend {
        expected_revision: claimed.revision,
        workspace: Box::new(failed.clone()),
        event_at: MINUTE.into(),
        event: SandboxEventKind::WorkspaceFailed {
            workspace_id: claimed.id.clone(),
            failure,
        },
        repository_runtime: None,
    };
    let result = WorkAuthority::new(
        ClaimedWork::Workspace(Box::new(claimed.clone())),
        Instant::now(),
    )
    .with_parent(parent.clone())
    .unwrap()
    .run(
        async {
            f.fail(command, 1, false).await;
            let work = f.store.commit(transaction);
            if scoped {
                crate::store::work_publication::scope(&f.store, work).await
            } else {
                work.await
            }
        },
        || async {
            f.independent
                .renew_workspace_authority(&claimed, &now(), &work_lease_deadline())
                .await
        },
    )
    .await;
    f.clear().await;
    let starts = f.starts();
    let stored = f.independent.get_workspace(&claimed.id).await.unwrap();
    let events = f
        .independent
        .sandbox_events(&parent.id, 0, 10)
        .await
        .unwrap();
    let stored_parent = f.independent.get_sandbox(&parent.id).await.unwrap();
    f.drop_database().await;
    result.expect("Workspace parent authority abort must retain its driver retry label");
    assert_fresh_transactions(&starts, 2);
    assert_eq!(stored, Some(failed));
    assert_eq!(stored_parent, Some(parent));
    assert_eq!(events.len(), 1);
}

async fn nested_child_claim_retries_aborted_original_parent_authority_case(scoped: bool) {
    let f = Fixture::new().await;
    let parent = sandbox("sbx_nested_fence");
    f.independent.create_sandbox(parent.clone()).await.unwrap();
    let child = workspace(&parent);
    f.independent.create_workspace(child.clone()).await.unwrap();
    let claimed = f
        .independent
        .claim_sandbox_work(
            &parent.id,
            "parent-worker",
            MINUTE.into(),
            work_lease_deadline(),
        )
        .await
        .unwrap()
        .unwrap();
    let result = WorkAuthority::new(
        ClaimedWork::Sandbox(Box::new(claimed.clone())),
        Instant::now(),
    )
    .run(
        async {
            f.fail("update", 1, false).await;
            let work = f.store.claim_workspace_work(
                &child.id,
                "child-worker",
                MINUTE.into(),
                work_lease_deadline(),
            );
            if scoped {
                crate::store::work_publication::scope(&f.store, work).await
            } else {
                work.await
            }
        },
        || async {
            f.independent
                .renew_sandbox_authority(&claimed, &now(), &work_lease_deadline())
                .await
        },
    )
    .await;
    f.clear().await;
    let starts = f.starts();
    let stored = f
        .independent
        .get_workspace(&child.id)
        .await
        .unwrap()
        .unwrap();
    let stored_parent = f
        .independent
        .get_sandbox(&parent.id)
        .await
        .unwrap()
        .unwrap();
    f.drop_database().await;
    let returned = result
        .expect("child claim must retry known-aborted parent authority fence")
        .expect("eligible child is claimed once");
    assert_fresh_transactions(&starts, 2);
    assert_eq!(stored, returned);
    assert_eq!(stored.revision, child.revision + 1);
    assert_eq!(stored.work.lease.as_ref().unwrap().fence, 1);
    assert_eq!(stored_parent.revision, claimed.revision);
    assert_eq!(
        stored_parent.work.lease.as_ref().unwrap().holder,
        "parent-worker"
    );
}

#[path = "workspace_cases.rs"]
mod cases;
