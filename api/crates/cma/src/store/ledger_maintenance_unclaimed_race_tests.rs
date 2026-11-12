use super::*;
use crate::store::agent_commands::ApplyError;
use crate::store::sandbox_mongo::{is_transient, start_transaction};
use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal;
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError;

#[tokio::test]
#[ignore = "requires an owned disposable replica set and unclaimed hooks"]
async fn hold_invalidates_an_unclaimed_settlement_snapshot_and_fresh_retry() {
    for v1 in [false, true] {
        let (client, name, store) = database().await;
        let (parent, command) = reserved(&store).await;
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        store
            .agent_commands
            .find_one(doc! {"_id":command.receipt.envelope.command_id.as_str()})
            .session(&mut session)
            .await
            .unwrap()
            .unwrap();
        let other = MongoSandboxStore::open(&client, &name).await.unwrap();
        maintenance::hold(&other, &parent, v1).await;
        let held = all_rows(&store).await;
        assert!(
            matches!(crate::store::ledger_maintenance_unclaimed::apply(&store, &command, command.revision,
            CommandFailureCode::AuthorityRevoked, NOW, &mut session).await,
            Err(ApplyError::Driver(error)) if is_transient(&error))
        );
        let _ = session.abort_transaction().await;
        assert_eq!(
            reject(&store, &command).await,
            Err(CommandLedgerError::NotAdmissible)
        );
        assert_eq!(all_rows(&store).await, held);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and unclaimed hooks"]
async fn unclaimed_settlement_invalidates_an_admission_snapshot_and_old_source() {
    use crate::store::ledger_maintenance::ApplyError as HoldError;
    use crate::store::ledger_maintenance_pre_exclusion::Change;
    let (client, name, store) = database().await;
    let (parent, command) = reserved(&store).await;
    let (policy, input) = support::pre_exclusion(&store, &parent, NOW).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    crate::store::ledger_maintenance_snapshot::load_pre_exclusion(
        &store,
        &input.caller,
        &parent.id,
        &mut session,
    )
    .await
    .unwrap_or_else(|_| panic!("fixture snapshot failed"));
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    reject(&other, &command).await.unwrap();
    assert_settled_once(&store, &command).await;
    let settled = all_rows(&store).await;
    assert!(
        matches!(crate::store::ledger_maintenance_pre_exclusion_transaction::apply(
        &store, &policy, &Change::Admit(input.clone()), &mut session).await,
        Err(HoldError::Driver(error)) if is_transient(&error))
    );
    let _ = session.abort_transaction().await;
    assert_eq!(
        store.admit_pre_exclusion(policy, input).await,
        Err(LedgerMaintenanceStoreError::Refused(
            LedgerMaintenanceAdmissionRefusal::Conflict
        ))
    );
    assert_eq!(all_rows(&store).await, settled);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set with failpoints and unclaimed hooks"]
async fn unknown_unclaimed_commit_never_reexecutes_or_claims_confirmed_success() {
    for committed in [false, true] {
        let (client, name, store) = database().await;
        let (parent, command) = reserved(&store).await;
        let before = all_rows(&store).await;
        let fence = maintenance::fence_value(&store, &parent).await;
        let data = if committed {
            doc! {"failCommands":["commitTransaction"], "writeConcernError":{"code":50,"codeName":"MaxTimeMSExpired","errmsg":"injected ambiguous refusal"}}
        } else {
            doc! {"failCommands":["commitTransaction"],"errorCode":251,"errorLabels":["UnknownTransactionCommitResult"]}
        };
        client
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":5},"data":data})
            .await
            .unwrap();
        let result = reject(&store, &command).await;
        support::disable_failpoint_and_observe_majority(&store).await;
        assert_eq!(result, Err(CommandLedgerError::Unavailable));
        if committed {
            assert_settled_once(&store, &command).await;
            assert_eq!(maintenance::fence_value(&store, &parent).await, fence + 1);
            let settled = all_rows(&store).await;
            assert_eq!(
                reject(&store, &command).await,
                Err(CommandLedgerError::StaleClaim)
            );
            assert_eq!(all_rows(&store).await, settled);
        } else {
            assert_eq!(all_rows(&store).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}
