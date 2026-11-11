//! Registered beneath the real integrity quarantine tests by A's hook patch.
use super::*;
use crate::store::ledger_maintenance_test_support as support;
use crate::store::sandbox_maintenance::mongo_integration_tests::database;
use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionPolicy;
use layer2_core::sandbox::store::ledger_maintenance::{
    LedgerMaintenanceAdmission, LedgerMaintenanceStore,
};
use layer2_core::sandbox::store::ledger_maintenance_pre_exclusion::LedgerPreExclusionStore;

#[path = "ledger_maintenance_integrity_preparation_tests.rs"]
mod preparation;

async fn row(store: &MongoSandboxStore, agent: &Agent) -> Document {
    store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap()
}

async fn hold(store: &MongoSandboxStore, mut input: LedgerMaintenanceAdmission, v1: bool) {
    if v1 {
        let mut plan = input.accepted.plan().clone();
        plan.source = store
            .inspect_ledger_maintenance(&input.caller, &plan.source.sandbox.id)
            .await
            .unwrap()
            .source;
        input.accepted = support::artifact(&plan);
        store
            .admit_ledger_maintenance(LedgerMaintenanceAdmissionPolicy { enabled: true }, input)
            .await
            .unwrap();
    } else {
        let (policy, input) =
            support::pre_exclusion(store, &input.accepted.plan().source.sandbox, support::NOW)
                .await;
        store.admit_pre_exclusion(policy, input).await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and integrity hooks"]
async fn quarantine_positive_control_preserves_a_newer_revision_of_the_same_history() {
    let (client, name, store) = database().await;
    let input = support::fixture(&store, true).await;
    let mut agent = input.accepted.plan().source.agents[0].clone();
    let observed = row(&store, &agent).await;
    let revision = agent.revision;
    agent.revision += 1;
    store.save_agent(revision, agent.clone()).await.unwrap();
    assert!(proof::quarantine_row(&store, &observed).await.is_ok());
    let current = row(&store, &agent).await;
    assert_eq!(current.get_i64("revision").unwrap(), agent.revision as i64);
    let marker = current.get_document(QUARANTINE_FIELD).unwrap();
    assert_eq!(marker.get_i64("observed_head").unwrap(), 1);
    assert_eq!(
        marker.get_str("reason").unwrap(),
        "integrity_validation_failed"
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and integrity hooks"]
async fn quarantine_refuses_both_holds_after_reopen_and_missing_audits_without_writes() {
    for v1 in [false, true] {
        let (client, name, store) = database().await;
        let input = support::fixture(&store, true).await;
        let observed = row(&store, &input.accepted.plan().source.agents[0]).await;
        hold(&store, input, v1).await;
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        let before = support::all_rows(&store).await;
        assert!(matches!(
            proof::quarantine_row(&reopened, &observed).await,
            Err(TransactionLogError::Domain(SandboxStoreError::Conflict))
        ));
        assert_eq!(support::all_rows(&store).await, before);
        let audits = if v1 {
            crate::store::ledger_maintenance_transaction::audit_collection(&store)
        } else {
            crate::store::ledger_maintenance_pre_exclusion_transaction::audit_collection(&store)
        };
        audits.delete_many(doc! {}).await.unwrap();
        let before = support::all_rows(&store).await;
        assert!(matches!(
            proof::quarantine_row(&reopened, &observed).await,
            Err(TransactionLogError::Domain(SandboxStoreError::Corrupt(_)))
        ));
        assert_eq!(support::all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and integrity hooks"]
async fn delayed_quarantine_refuses_replaced_or_moved_history_without_parent_writes() {
    for case in 0..4 {
        let (client, name, store) = database().await;
        let input = support::fixture(&store, true).await;
        let mut agent = input.accepted.plan().source.agents[0].clone();
        let observed = row(&store, &agent).await;
        match case {
            0 => agent.creation_key.push_str("-replacement"),
            1 => agent.owner = layer2_core::SubjectId("other-fixture-principal".into()),
            2 => agent.sandbox_id = layer2_core::SandboxId::generate(),
            _ => agent.worktree_id = layer2_core::WorktreeId::generate(),
        }
        store
            .agents
            .replace_one(
                doc! {"_id":agent.id.as_str()},
                crate::store::agents::agent_document(&agent).unwrap(),
            )
            .await
            .unwrap();
        let before = support::all_rows(&store).await;
        assert!(proof::quarantine_row(&store, &observed).await.is_err());
        assert_eq!(support::all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and integrity hooks"]
async fn quarantine_update_failure_rolls_back_the_parent_barrier_and_then_can_succeed() {
    let (client, name, store) = database().await;
    let input = support::fixture(&store, true).await;
    let agent = &input.accepted.plan().source.agents[0];
    let observed = row(&store, agent).await;
    client
        .database(&name)
        .run_command(doc! {
            "collMod":store.agents.name(),
            "validator":{"reject_fixture":{"$exists":true}},
            "validationLevel":"strict", "validationAction":"error",
        })
        .await
        .unwrap();
    let before = support::all_rows(&store).await;
    assert!(proof::quarantine_row(&store, &observed).await.is_err());
    assert_eq!(support::all_rows(&store).await, before);
    client
        .database(&name)
        .run_command(doc! {"collMod":store.agents.name(),"validator":{}})
        .await
        .unwrap();
    assert!(proof::quarantine_row(&store, &observed).await.is_ok());
    assert!(row(&store, agent).await.contains_key(QUARANTINE_FIELD));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and integrity hooks"]
async fn orphan_quarantine_refuses_without_guessing_the_original_home_is_unheld() {
    let (client, name, store) = database().await;
    let input = support::fixture(&store, true).await;
    let observed = row(&store, &input.accepted.plan().source.agents[0]).await;
    store.sandboxes.delete_many(doc! {}).await.unwrap();
    let before = support::all_rows(&store).await;
    assert!(matches!(
        proof::quarantine_row(&store, &observed).await,
        Err(TransactionLogError::Domain(SandboxStoreError::NotFound))
    ));
    assert_eq!(support::all_rows(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set with failpoints and integrity hooks"]
async fn failed_quarantine_commit_does_not_publish_the_marker_or_parent_fence() {
    let (client, name, store) = database().await;
    let input = support::fixture(&store, true).await;
    let observed = row(&store, &input.accepted.plan().source.agents[0]).await;
    let before = support::all_rows(&store).await;
    let admin =
        mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":1},
            "data":{"failCommands":["commitTransaction"],"errorCode":13},
        })
        .await
        .unwrap();
    let outcome = proof::quarantine_row(&store, &observed).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(outcome.is_err());
    assert_eq!(support::all_rows(&store).await, before);
    assert!(proof::quarantine_row(&store, &observed).await.is_ok());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set with failpoints and integrity hooks"]
async fn unknown_quarantine_commit_does_not_start_another_write_or_report_confirmation() {
    for committed in [false, true] {
        let (client, name, store) = database().await;
        let input = support::fixture(&store, true).await;
        let source = &input.accepted.plan().source;
        let observed = row(&store, &source.agents[0]).await;
        let before = support::all_rows(&store).await;
        let data = if committed {
            doc! {"failCommands":["commitTransaction"], "writeConcernError":{
                "code":50, "codeName":"MaxTimeMSExpired",
                "errmsg":"injected uncertain integrity commit",
            }}
        } else {
            doc! {"failCommands":["commitTransaction"], "errorCode":251,
            "errorLabels":["UnknownTransactionCommitResult"]}
        };
        client
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":5},"data":data})
            .await
            .unwrap();
        let outcome = proof::quarantine_row(&store, &observed).await;
        client
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
        assert!(matches!(outcome, Err(TransactionLogError::Driver(error))
            if error.contains_label("UnknownTransactionCommitResult")));
        if committed {
            assert!(row(&store, &source.agents[0])
                .await
                .contains_key(QUARANTINE_FIELD));
            let parent = store
                .sandboxes
                .find_one(doc! {"_id":source.sandbox.id.as_str()})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                parent.get_i64("ledger_writer_admission_fence_v1").unwrap(),
                1
            );
        } else {
            // Restarting the logical update after the fifth failed commit would
            // succeed once the failpoint is exhausted and make this comparison fail.
            assert_eq!(support::all_rows(&store).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and integrity hooks"]
async fn committed_quarantine_invalidates_an_inflight_maintenance_snapshot() {
    use crate::store::ledger_maintenance::{ApplyError, Change};
    use crate::store::sandbox_mongo::{is_transient, start_transaction};
    let (client, name, store) = database().await;
    let input = support::fixture(&store, true).await;
    let source = &input.accepted.plan().source;
    let observed = row(&store, &source.agents[0]).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    store
        .sandboxes
        .find_one(doc! {"_id":source.sandbox.id.as_str()})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert!(proof::quarantine_row(&other, &observed).await.is_ok());
    let committed = support::all_rows(&store).await;
    let change = Change::Admit(LedgerMaintenanceAdmissionPolicy { enabled: true }, input);
    let result =
        crate::store::ledger_maintenance_transaction::apply(&store, &change, &mut session).await;
    assert!(matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)));
    let _ = session.abort_transaction().await;
    assert_eq!(support::all_rows(&store).await, committed);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and integrity hooks"]
async fn committed_hold_invalidates_an_inflight_quarantine_transaction() {
    use crate::store::sandbox_mongo::{is_transient, start_transaction};
    let (client, name, store) = database().await;
    let input = support::fixture(&store, true).await;
    let source = &input.accepted.plan().source;
    let observed = source.agents[0].clone();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    store
        .sandboxes
        .find_one(doc! {"_id":source.sandbox.id.as_str()})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    hold(&other, input, true).await;
    let committed = support::all_rows(&store).await;
    let result = crate::store::ledger_maintenance_integrity::apply_quarantine(
        &store,
        &observed,
        &doc! {"_id":observed.id.as_str()},
        &doc! {"$set":{QUARANTINE_FIELD:{
            "format":1_i64,"reason":"integrity_validation_failed","observed_head":1_i64,
        }}},
        &mut session,
    )
    .await;
    assert!(matches!(result, Err(TransactionLogError::Driver(error)) if is_transient(&error)));
    let _ = session.abort_transaction().await;
    assert_eq!(support::all_rows(&store).await, committed);
    client.database(&name).drop().await.unwrap();
}
