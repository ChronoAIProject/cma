use super::*;
use crate::store::ledger_maintenance::ApplyError;
use crate::store::ledger_maintenance_pre_exclusion::Change;
use crate::store::ledger_maintenance_pre_exclusion_transaction as transaction;
use crate::store::ledger_maintenance_snapshot;
use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal;
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError;

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and child-save hooks"]
async fn child_commit_invalidates_an_independent_open_maintenance_snapshot() {
    for agent in [false, true] {
        let (client, name, store) = database().await;
        let input = support::fixture(&store, true).await;
        let source = input.accepted.plan().source.clone();
        let (policy, input) = support::pre_exclusion(&store, &source.sandbox, NOW).await;
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        ledger_maintenance_snapshot::load_pre_exclusion(
            &store,
            &input.caller,
            &source.sandbox.id,
            &mut session,
        )
        .await
        .unwrap_or_else(|_| panic!("fixture snapshot failed"));
        let other = MongoSandboxStore::open(&client, &name).await.unwrap();
        port(&other, agent, &next(&source)).await.unwrap();
        let before = all_rows(&store).await;
        assert!(
            matches!(transaction::apply(&store, &policy, &Change::Admit(input.clone()), &mut session).await,
            Err(ApplyError::Driver(error)) if is_transient(&error))
        );
        let _ = session.abort_transaction().await;
        assert_eq!(
            store.admit_pre_exclusion(policy, input).await,
            Err(LedgerMaintenanceStoreError::Refused(
                LedgerMaintenanceAdmissionRefusal::Conflict
            ))
        );
        assert_eq!(all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and child-save hooks"]
async fn hold_commit_invalidates_an_independent_open_child_save_snapshot() {
    for agent in [false, true] {
        let (client, name, store) = database().await;
        let input = support::fixture(&store, true).await;
        let source = input.accepted.plan().source.clone();
        let next = next(&source);
        let (policy, input) = support::pre_exclusion(&store, &source.sandbox, NOW).await;
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        ledger_maintenance_snapshot::load_pre_exclusion(
            &store,
            &input.caller,
            &source.sandbox.id,
            &mut session,
        )
        .await
        .unwrap_or_else(|_| panic!("fixture snapshot failed"));
        let other = MongoSandboxStore::open(&client, &name).await.unwrap();
        other.admit_pre_exclusion(policy, input).await.unwrap();
        let before = all_rows(&store).await;
        let expected = if agent {
            next.0.revision - 1
        } else {
            next.1.revision - 1
        };
        assert!(
            matches!(apply(&store, expected, &mutation(agent, &next), &mut session).await,
            Err(Error::Driver(error)) if is_transient(&error))
        );
        let _ = session.abort_transaction().await;
        assert_eq!(
            port(&store, agent, &next).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and child-save hooks"]
async fn child_write_failure_rolls_back_its_parent_fence_and_retry_succeeds() {
    for agent in [false, true] {
        let (client, name, store) = database().await;
        let input = support::fixture(&store, true).await;
        let next = next(&input.accepted.plan().source);
        let collection = if agent {
            &store.agents
        } else {
            &store.workspaces
        };
        client.database(&name).run_command(doc! {"collMod":collection.name(),"validator":{"reject_fixture":{"$exists":true}},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
        let before = all_rows(&store).await;
        assert!(matches!(
            port(&store, agent, &next).await,
            Err(SandboxStoreError::Unavailable(_))
        ));
        assert_eq!(all_rows(&store).await, before);
        client
            .database(&name)
            .run_command(doc! {"collMod":collection.name(),"validator":{}})
            .await
            .unwrap();
        port(&store, agent, &next).await.unwrap();
        assert_saved(&store, agent, &next).await;
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set with test commands and child-save hooks"]
async fn failed_commit_cannot_leave_a_child_or_parent_write() {
    for agent in [false, true] {
        let (client, name, store) = database().await;
        let input = support::fixture(&store, true).await;
        let next = next(&input.accepted.plan().source);
        let before = all_rows(&store).await;
        client
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},
            "data":{"failCommands":["commitTransaction"],"errorCode":13}})
            .await
            .unwrap();
        let result = port(&store, agent, &next).await;
        client
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
        assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
        assert_eq!(all_rows(&store).await, before);
        port(&store, agent, &next).await.unwrap();
        assert_saved(&store, agent, &next).await;
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set with test commands and child-save hooks"]
async fn unknown_commit_never_restarts_save_or_claims_confirmed_success() {
    for agent in [false, true] {
        for committed in [false, true] {
            let (client, name, store) = database().await;
            let input = support::fixture(&store, true).await;
            let next = next(&input.accepted.plan().source);
            let before = all_rows(&store).await;
            let data = if committed {
                doc! {"failCommands":["commitTransaction"],"writeConcernError":{"code":50,"codeName":"MaxTimeMSExpired","errmsg":"injected ambiguous child save"}}
            } else {
                doc! {"failCommands":["commitTransaction"],"errorCode":251,"errorLabels":["UnknownTransactionCommitResult"]}
            };
            client
                .database("admin")
                .run_command(
                    doc! {"configureFailPoint":"failCommand","mode":{"times":5},"data":data},
                )
                .await
                .unwrap();
            let result = port(&store, agent, &next).await;
            support::disable_failpoint_and_observe_majority(&store).await;
            let Err(SandboxStoreError::Unavailable(error)) = result else {
                panic!("uncertain save must not report confirmed success");
            };
            assert!(
                error.contains("UnknownTransactionCommitResult"),
                "wrong failure path: agent={agent}, expected_committed={committed}, write_conflict={}, missing_transaction={}",
                error.contains("WriteConflict"), error.contains("NoSuchTransaction"),
            );
            if committed {
                assert_saved(&store, agent, &next).await;
                let after = all_rows(&store).await;
                assert_eq!(
                    port(&store, agent, &next).await,
                    Err(SandboxStoreError::Conflict)
                );
                assert_eq!(all_rows(&store).await, after);
            } else {
                assert_eq!(all_rows(&store).await, before);
                port(&store, agent, &next).await.unwrap();
                assert_saved(&store, agent, &next).await;
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}
