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
async fn held_workspace_cannot_take_discovery_slots_or_obtain_worker_authority() {
    use crate::store::work_claim_authority::LEDGER_MAINTENANCE_FIELD;
    use mongodb::bson::Bson;

    let fixture = Fixture::new().await;
    let parent = sandbox("sbx_held_workspace");
    let mut sibling = sandbox("sbx_unheld_workspace");
    sibling.name = "unheld workspace parent".into();
    fixture
        .independent
        .create_sandbox(parent.clone())
        .await
        .unwrap();
    fixture
        .independent
        .create_sandbox(sibling.clone())
        .await
        .unwrap();
    let mut renew = workspace(&parent);
    renew.id = WorkspaceId("wks_held_renew".into());
    renew.name = "held-renew".into();
    renew.path = "/home/cma/held-renew".into();
    let pending = workspace(&parent);
    let mut unheld = workspace(&sibling);
    unheld.id = WorkspaceId("wks_unheld".into());
    for row in [&renew, &pending, &unheld] {
        fixture
            .independent
            .create_workspace(row.clone())
            .await
            .unwrap();
    }
    let claimed = fixture
        .store
        .claim_workspace_work(&renew.id, "worker", MINUTE.into(), work_lease_deadline())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        fixture.store.due_workspace_work(MINUTE, 1).await.unwrap(),
        vec![pending.id.clone()]
    );
    let filter = doc! {"_id": parent.id.as_str()};
    for hold in [Bson::Document(doc! {"phase":"requested"}), Bson::Null] {
        fixture
            .independent
            .sandboxes
            .update_one(
                filter.clone(),
                doc! {"$set": {LEDGER_MAINTENANCE_FIELD: hold.clone()}},
            )
            .await
            .unwrap();
        let held_parent = fixture
            .independent
            .sandboxes
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(held_parent.get(LEDGER_MAINTENANCE_FIELD), Some(&hold));
        assert_eq!(
            fixture.store.due_workspace_work(MINUTE, 1).await.unwrap(),
            vec![unheld.id.clone()]
        );
        assert!(fixture
            .store
            .claim_workspace_work(
                &pending.id,
                "successor",
                MINUTE.into(),
                work_lease_deadline()
            )
            .await
            .unwrap()
            .is_none());
        assert!(fixture
            .store
            .renew_workspace_authority(&claimed, MINUTE, &work_lease_deadline())
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            fixture
                .independent
                .sandboxes
                .find_one(filter.clone())
                .await
                .unwrap(),
            Some(held_parent)
        );
        assert_eq!(
            fixture
                .independent
                .get_workspace(&pending.id)
                .await
                .unwrap()
                .as_ref(),
            Some(&pending)
        );
        assert_eq!(
            fixture
                .independent
                .get_workspace(&renew.id)
                .await
                .unwrap()
                .as_ref(),
            Some(&claimed)
        );
    }
    assert!(fixture
        .store
        .claim_workspace_work(
            &unheld.id,
            "successor",
            MINUTE.into(),
            work_lease_deadline()
        )
        .await
        .unwrap()
        .is_some());
    fixture.drop_database().await;
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
