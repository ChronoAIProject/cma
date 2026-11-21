use super::*;
use crate::store::sandboxes::sandbox_document;
use crate::store::workspaces::workspace_document;

#[derive(Clone, Copy)]
enum Replacement {
    Runtime,
    Credential,
    ResolvedCommit,
    PendingChild,
    Cleanup,
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_recovery_revalidates_original_authority_after_an_aborted_transaction() {
    for replacement in [
        Replacement::Runtime,
        Replacement::Credential,
        Replacement::ResolvedCommit,
        Replacement::PendingChild,
        Replacement::Cleanup,
    ] {
        let fixture = Fixture::new(false, 1).await;
        let mut captured = RecoveryParents::default();
        let at = layer2_core::now_rfc3339();
        let mut session = fixture
            .controller
            .store
            .client
            .start_session()
            .await
            .unwrap();
        start_transaction(&mut session).await.unwrap();
        let plan = fixture
            .controller
            .recover_in(
                &fixture.binding,
                Some(&fixture.key),
                &at,
                &mut captured,
                &mut session,
            )
            .await;
        assert!(
            plan.is_ok(),
            "the original recovered hierarchy can be published"
        );
        session.abort_transaction().await.unwrap();

        replace_parent(&fixture, replacement).await;

        let store = &fixture.controller.store;
        let mut retry = store.client.start_session().await.unwrap();
        start_transaction(&mut retry).await.unwrap();
        let result = fixture
            .controller
            .recover_in(
                &fixture.binding,
                Some(&fixture.key),
                &at,
                &mut captured,
                &mut retry,
            )
            .await;
        assert!(matches!(
            result,
            Err(BindingTransactionError::Domain(CoreError::InvalidState(_)))
        ));
        retry.abort_transaction().await.unwrap();
        let snapshot = fixture.snapshot().await;
        assert_eq!(snapshot.0, fixture.binding);
        assert_eq!((snapshot.3, snapshot.4), (0, 0));
        fixture.cleanup().await;
    }
}

async fn replace_parent(fixture: &Fixture, replacement: Replacement) {
    let store = &fixture.controller.store;
    match replacement {
        Replacement::Runtime => {
            let mut sandbox = fixture.sandbox.clone();
            let runtime = sandbox.runtime.as_mut().unwrap();
            runtime.runtime_id = "replacement-runtime".into();
            runtime.provision_generation = Some(4);
            sandbox.lifecycle.runtime_generation_high_water = 4;
            sandbox.revision += 1;
            store
                .sandboxes
                .replace_one(
                    doc! {"_id":sandbox.id.as_str()},
                    sandbox_document(&sandbox, 0).unwrap(),
                )
                .await
                .unwrap();
        }
        Replacement::Credential => {
            let mut sandbox = fixture.sandbox.clone();
            let mut material = fixture.custody.get(&sandbox.id, 1).await.unwrap().unwrap();
            material.authority.generation = 2;
            fixture.custody.insert(material.clone()).await.unwrap();
            sandbox.credential.key_generation = 2;
            sandbox.credential.current = Some(material.authority);
            sandbox.revision += 1;
            store
                .sandboxes
                .replace_one(
                    doc! {"_id": sandbox.id.as_str()},
                    sandbox_document(&sandbox, 0).unwrap(),
                )
                .await
                .unwrap();
        }
        Replacement::ResolvedCommit => {
            let mut workspace = fixture.workspace.clone();
            workspace.resolved_commit = Some("f".repeat(40));
            workspace.revision += 1;
            store
                .workspaces
                .replace_one(
                    doc! {"_id":workspace.id.as_str()},
                    workspace_document(&workspace).unwrap(),
                )
                .await
                .unwrap();
        }
        Replacement::PendingChild => {
            store.worktrees.insert_one(doc! {
                    "_id": "new-pending-worktree", "sandbox_id": fixture.sandbox.id.as_str(), "status": "creating",
                }).await.unwrap();
        }
        Replacement::Cleanup => {
            store
                .workspaces
                .update_one(
                    doc! {"_id":fixture.workspace.id.as_str()},
                    doc! {"$set": {"owned_workspace_cleanup_admission_v1": {"pending": true}}},
                )
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn binding_recovery_preserves_partial_sandbox_authority_across_workspace_read_abort() {
    for replacement in [
        None,
        Some(Replacement::Runtime),
        Some(Replacement::Credential),
    ] {
        let fixture = Fixture::new(true, 1).await;
        let store = &fixture.controller.store;
        let at = layer2_core::now_rfc3339();
        let mut captured = RecoveryParents::default();
        let mut first = store.client.start_session().await.unwrap();
        start_transaction(&mut first).await.unwrap();
        // The first find reads the Sandbox. The next find reads the Workspace.
        fixture
            .admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": {"skip": 1_i32},
                "data": {"appName": &fixture.app, "failCommands": ["find"],
                    "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"]},
            })
            .await
            .unwrap();
        let original = fixture
            .controller
            .recover_in(
                &fixture.binding,
                Some(&fixture.key),
                &at,
                &mut captured,
                &mut first,
            )
            .await;
        fixture
            .admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": "off",
            })
            .await
            .unwrap();
        assert!(
            matches!(original, Err(BindingTransactionError::Driver(ref error)) if known_abort(error))
        );
        let _ = first.abort_transaction().await;

        if let Some(replacement) = replacement {
            replace_parent(&fixture, replacement).await;
        }
        let mut second = store.client.start_session().await.unwrap();
        start_transaction(&mut second).await.unwrap();
        let retried = fixture
            .controller
            .recover_in(
                &fixture.binding,
                Some(&fixture.key),
                &at,
                &mut captured,
                &mut second,
            )
            .await;
        let publish = replacement.is_none();
        if publish {
            assert!(retried.is_ok());
            assert!(matches!(
                commit_transaction(&mut second).await,
                CommitOutcome::Committed
            ));
        } else {
            second.abort_transaction().await.unwrap();
            assert!(matches!(
                retried,
                Err(BindingTransactionError::Domain(CoreError::InvalidState(_)))
            ));
        }
        let snapshot = fixture.snapshot().await;
        let workspace = store
            .workspaces
            .find_one(doc! {"_id": fixture.workspace.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let workspace_fence = workspace
            .get_i64("binding_recovery_admission_fence")
            .unwrap_or(0);
        fixture.cleanup().await;
        let expected_fence = i64::from(publish);
        assert_eq!(
            (snapshot.3, workspace_fence, snapshot.4),
            (expected_fence, expected_fence, expected_fence)
        );
        if publish {
            assert_eq!(snapshot.0.state, BindingState::Ready);
            assert_eq!(snapshot.0.updated_at, at);
        } else {
            assert_eq!(snapshot.0, fixture.binding);
        }
    }
}
