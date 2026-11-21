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
        let mut captured = None;
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
