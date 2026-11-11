//! Real creation ports, with shared integration hooks supplied by Track A.
use super::*;

#[path = "ledger_maintenance_creation_test_support.rs"]
mod creation;
use creation::*;

#[path = "ledger_maintenance_creation_reuse_tests.rs"]
mod reuse;

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and creation hooks"]
async fn new_agent_and_workspace_creation_have_positive_controls_and_held_zero_write_refusals() {
    for agent in [false, true] {
        for held in [false, true] {
            let (client, name, store) = database().await;
            let (policy, input) = fixture(&store).await;
            let request = request(&input.accepted.plan().source, agent);
            if held {
                store.admit_pre_exclusion(policy, input).await.unwrap();
            }
            let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
            let before = all_rows(&store).await;
            let result = create(&reopened, &request).await;
            if held {
                assert_eq!(result, Err(SandboxStoreError::Conflict));
                assert_eq!(all_rows(&store).await, before);
            } else {
                assert_created(&result.unwrap());
                assert_ne!(all_rows(&store).await, before);
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and creation hooks"]
async fn historical_unbound_agent_creation_replays_exactly_without_writes_under_hold() {
    use layer2_core::sandbox::store::AgentAdmissionResult;
    let (client, name, store) = database().await;
    let (policy, mut input) = fixture(&store).await;
    let request = request(&input.accepted.plan().source, true);
    let original = match create(&store, &request).await.unwrap() {
        Outcome::Agent(AgentAdmissionResult::Created(creation)) => creation,
        _ => panic!("fixture Agent was not newly admitted"),
    };
    refresh(&store, &mut input).await;
    store.admit_pre_exclusion(policy, input).await.unwrap();
    let before = all_rows(&store).await;
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        create(&reopened, &request).await.unwrap(),
        Outcome::Agent(AgentAdmissionResult::Existing(original))
    );
    assert_eq!(all_rows(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and creation hooks"]
async fn original_v1_maintenance_blocks_creation_without_a_v2_schema_filter() {
    use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionPolicy;
    use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStore;
    let (client, name, store) = database().await;
    let input = support::fixture(&store, true).await;
    let requests = [
        request(&input.accepted.plan().source, false),
        request(&input.accepted.plan().source, true),
    ];
    store
        .admit_ledger_maintenance(LedgerMaintenanceAdmissionPolicy { enabled: true }, input)
        .await
        .unwrap();
    assert_eq!(
        store
            .sandboxes
            .find_one(doc! {})
            .await
            .unwrap()
            .unwrap()
            .get_i64("document_schema_version")
            .unwrap(),
        1
    );
    let before = all_rows(&store).await;
    for request in requests {
        assert_eq!(
            create(&store, &request).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(all_rows(&store).await, before);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and creation hooks"]
async fn missing_or_malformed_maintenance_audit_refuses_creation_without_partial_fences() {
    for malformed in [false, true] {
        let (client, name, store) = database().await;
        let (policy, input) = fixture(&store).await;
        let requests = [
            request(&input.accepted.plan().source, false),
            request(&input.accepted.plan().source, true),
        ];
        store.admit_pre_exclusion(policy, input).await.unwrap();
        if malformed {
            store
                .sandboxes
                .update_one(doc! {}, doc! {"$set":{records::FIELD:"malformed-fixture"}})
                .await
                .unwrap();
        } else {
            transaction::audit_collection(&store)
                .delete_many(doc! {})
                .await
                .unwrap();
        }
        let before = all_rows(&store).await;
        for request in requests {
            assert!(matches!(
                create(&store, &request).await,
                Err(SandboxStoreError::Corrupt(_))
            ));
            assert_eq!(all_rows(&store).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and creation hooks"]
async fn failed_child_insert_rolls_back_creation_fences_and_all_sidecars() {
    for agent in [false, true] {
        let (client, name, store) = database().await;
        let (_, input) = fixture(&store).await;
        let request = request(&input.accepted.plan().source, agent);
        let collection = if agent {
            store.agents.name()
        } else {
            store.workspaces.name()
        };
        client.database(&name).run_command(doc! {"collMod":collection, "validator":{"reject_fixture":{"$exists":true}}, "validationLevel":"strict", "validationAction":"error"}).await.unwrap();
        let before = all_rows(&store).await;
        assert!(matches!(
            create(&store, &request).await,
            Err(SandboxStoreError::Unavailable(_))
        ));
        assert_eq!(all_rows(&store).await, before);
        client
            .database(&name)
            .run_command(doc! {"collMod":collection, "validator":{}})
            .await
            .unwrap();
        assert_created(&create(&store, &request).await.unwrap());
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and creation hooks"]
async fn creation_commits_conflict_with_an_already_open_maintenance_snapshot() {
    for agent in [false, true] {
        let (client, name, store) = database().await;
        let (policy, input) = fixture(&store).await;
        let request = request(&input.accepted.plan().source, agent);
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        ledger_maintenance_snapshot::load_pre_exclusion(
            &store,
            &input.caller,
            &input.accepted.plan().sandbox_id,
            &mut session,
        )
        .await
        .unwrap_or_else(|_| panic!("fixture snapshot failed"));
        let other = MongoSandboxStore::open(&client, &name).await.unwrap();
        assert_created(&create(&other, &request).await.unwrap());
        let before = all_rows(&store).await;
        assert!(
            matches!(transaction::apply(&store,&policy,&Change::Admit(input.clone()),&mut session).await,Err(ApplyError::Driver(error)) if is_transient(&error))
        );
        let _ = session.abort_transaction().await;
        assert_eq!(all_rows(&store).await, before);
        assert_eq!(
            store.admit_pre_exclusion(policy, input).await,
            Err(Domain::Refused(Refusal::Conflict))
        );
        assert_eq!(all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and creation hooks"]
async fn hold_commit_defeats_an_open_creation_writer_snapshot() {
    use crate::store::ledger_maintenance_creation_ports::{fence_workspace, CreationError};
    let (client, name, store) = database().await;
    let (policy, input) = fixture(&store).await;
    let source = input.accepted.plan().source.clone();
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
    assert!(
        matches!(fence_workspace(&store,&source.workspaces[0].id,&source.sandbox.owner,&mut session).await,Err(CreationError::Driver(error)) if is_transient(&error))
    );
    let _ = session.abort_transaction().await;
    assert_eq!(all_rows(&store).await, before);
    for agent in [false, true] {
        assert_eq!(
            create(&store, &request(&source, agent)).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(all_rows(&store).await, before);
    }
    client.database(&name).drop().await.unwrap();
}
