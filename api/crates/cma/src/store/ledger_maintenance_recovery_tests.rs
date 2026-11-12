//! Registered beneath retained_reopen::mongo_tests to exercise its transaction.
use super::*;
use crate::store::ledger_maintenance_publication::test_support as maintenance;
use crate::store::ledger_maintenance_test_support as support;
use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal;
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError;
use layer2_core::sandbox::store::ledger_maintenance_pre_exclusion::LedgerPreExclusionStore;

#[path = "ledger_maintenance_recovery_race_tests.rs"]
mod races;

#[derive(Clone)]
enum Recovery {
    Retained(Box<RetainedReopenRequest>),
    Unpublished(Box<SandboxUnpublishedRecoveryRequest>),
}

impl Recovery {
    async fn fixture(store: &MongoSandboxStore, retained: bool) -> Self {
        if retained {
            Self::Retained(Box::new(reopen_fixture(store, "sbx_recovery_hold").await))
        } else {
            let mut request =
                crate::store::sandbox_unpublished_recovery::mongo_integration_tests::fixture(
                    store,
                    "sbx_recovery_hold",
                )
                .await;
            // Maintenance requires a complete hierarchy, not just the legacy
            // recovery fixture's parent/Agent projection.
            support::insert_hierarchy(
                store,
                &request.snapshot.sandbox,
                &request.snapshot.agents[0],
            )
            .await;
            request.snapshot = store
                .unpublished_runtime_recovery_snapshot(&request.snapshot.sandbox.id)
                .await
                .unwrap();
            Self::Unpublished(Box::new(request))
        }
    }

    fn parent(&self) -> &Sandbox {
        match self {
            Self::Retained(request) => &request.snapshot.retained.sandbox,
            Self::Unpublished(request) => &request.snapshot.sandbox,
        }
    }

    async fn apply(&self, store: &MongoSandboxStore) -> Result<(), SandboxStoreError> {
        match self {
            Self::Retained(request) => {
                let (expected, receipt, _) =
                    apply_retained_reopen(&request.snapshot, request).unwrap();
                let result = store.admit_retained_reopen(*request.clone()).await?;
                assert_eq!(result, receipt.accepted);
                assert!(store.get_sandbox(&expected.id).await.unwrap() == Some(expected));
                assert_eq!(
                    store
                        .retained_reopen_receipt(&receipt.sandbox_id)
                        .await
                        .unwrap(),
                    Some(receipt)
                );
                for agent in &request.snapshot.retained.agents {
                    assert!(store.get_agent(&agent.id).await.unwrap() == Some(agent.clone()));
                }
            }
            Self::Unpublished(request) => {
                let expected = apply_unpublished_runtime_recovery(&request.snapshot, request)
                    .unwrap()
                    .sandbox;
                let actual = store
                    .admit_unpublished_runtime_recovery(*request.clone())
                    .await?;
                assert!(actual == expected);
                assert!(store.get_sandbox(&expected.id).await.unwrap() == Some(expected));
                for agent in &request.snapshot.agents {
                    assert!(store.get_agent(&agent.id).await.unwrap() == Some(agent.clone()));
                }
            }
        }
        assert_eq!(
            store
                .sandbox_events(&self.parent().id, 0, 100)
                .await
                .unwrap()
                .len(),
            1
        );
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and recovery hooks"]
async fn fresh_recovery_succeeds_unheld_and_refuses_both_holds_after_reopen() {
    for retained in [false, true] {
        for hold in [0, 1, 2] {
            let (client, name, store) = database().await;
            let request = Recovery::fixture(&store, retained).await;
            let fence = maintenance::fence_value(&store, request.parent()).await;
            if hold != 0 {
                maintenance::hold(&store, request.parent(), hold == 1).await;
            }
            let before = support::all_rows(&store).await;
            let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
            let result = request.apply(&reopened).await;
            if hold == 0 {
                result.unwrap();
                assert_ne!(support::all_rows(&store).await, before);
                assert_eq!(
                    maintenance::fence_value(&store, request.parent()).await,
                    fence + 1
                );
            } else {
                assert_eq!(result, Err(SandboxStoreError::Conflict));
                assert_eq!(support::all_rows(&store).await, before);
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and recovery hooks"]
async fn recorded_reopen_replay_remains_an_exact_read_under_both_holds() {
    for v1 in [false, true] {
        let (client, name, store) = database().await;
        let request = reopen_fixture(&store, "sbx_recovery_replay_hold").await;
        let accepted = store.admit_retained_reopen(request.clone()).await.unwrap();
        let parent = store
            .get_sandbox(&request.snapshot.retained.sandbox.id)
            .await
            .unwrap()
            .unwrap();
        maintenance::hold(&store, &parent, v1).await;
        let before = support::all_rows(&store).await;
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        assert_eq!(
            reopened
                .admit_retained_reopen(request.clone())
                .await
                .unwrap(),
            accepted
        );
        assert_eq!(support::all_rows(&store).await, before);
        let mut reused = request;
        reused.key_sha256 = "f".repeat(64);
        assert_eq!(
            reopened.admit_retained_reopen(reused).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(support::all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and recovery hooks"]
async fn recovery_refuses_missing_hold_audit_without_any_partial_changes() {
    for retained in [false, true] {
        for v1 in [false, true] {
            let (client, name, store) = database().await;
            let request = Recovery::fixture(&store, retained).await;
            maintenance::hold(&store, request.parent(), v1).await;
            let audit = if v1 {
                crate::store::ledger_maintenance_transaction::audit_collection(&store)
            } else {
                crate::store::ledger_maintenance_pre_exclusion_transaction::audit_collection(&store)
            };
            audit.delete_many(doc! {}).await.unwrap();
            let before = support::all_rows(&store).await;
            assert!(matches!(
                request.apply(&store).await,
                Err(SandboxStoreError::Corrupt(_))
            ));
            assert_eq!(support::all_rows(&store).await, before);
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set with failpoints and recovery hooks"]
async fn failed_recovery_commit_rolls_back_all_state_before_an_exact_retry() {
    for retained in [false, true] {
        let (client, name, store) = database().await;
        let request = Recovery::fixture(&store, retained).await;
        let before = support::all_rows(&store).await;
        client.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},"data":{"failCommands":["commitTransaction"],"errorCode":13}}).await.unwrap();
        let result = request.apply(&store).await;
        client
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
        assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
        assert_eq!(support::all_rows(&store).await, before);
        support::observe_transaction_cleanup(&store).await;
        assert_eq!(support::all_rows(&store).await, before);
        request.apply(&store).await.unwrap();
        client.database(&name).drop().await.unwrap();
    }
}
