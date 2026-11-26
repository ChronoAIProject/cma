use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::database;
use crate::store::sandbox_mongo_integration_tests::work_lease_deadline;
use layer2_core::sandbox::store::{AgentStore, SandboxStore};
use layer2_core::sandbox::*;
const NOW: &str = "2026-09-05T00:30:00Z";
use crate::store::agent_log_segments::transaction_scope::TransactionScope;
use layer2_core::sandbox::store::{SandboxEventStore, SandboxWorkerStore};

use super::mongo_fixtures::reopen_fixture;

#[path = "mongo_refusal_tests.rs"]
mod refusals;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_reopen_mongo_two_instances_preserve_receipt_and_old_reader_fields() {
    let (client, name, store) = database().await;
    let request = reopen_fixture(&store, "sbx_reopen_once").await;
    let id = &request.snapshot.retained.sandbox.id;
    let before = request.snapshot.retained.agents.clone();
    let second = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (a, b) = tokio::join!(
        store.admit_retained_reopen(request.clone()),
        second.admit_retained_reopen(request.clone())
    );
    let accepted = a.unwrap();
    assert_eq!(accepted, b.unwrap());
    assert_eq!(store.sandbox_events(id, 0, 100).await.unwrap().len(), 1);
    for agent in &before {
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap().as_ref(),
            Some(agent)
        );
        assert_eq!(
            store
                .agent_log
                .count_documents(doc! {"agent_id": agent.id.as_str()})
                .await
                .unwrap(),
            3
        );
    }
    let receipt = store.retained_reopen_receipt(id).await.unwrap().unwrap();
    let parent = store.get_sandbox(id).await.unwrap().unwrap();
    assert_eq!(
        receipt.proof.expected_runtime,
        request.snapshot.retained.sandbox.runtime.clone().unwrap()
    );
    assert_eq!(
        parent.runtime.as_ref().unwrap().runtime_version,
        request.proof.expected_runtime.runtime_version
    );
    assert_eq!(
        parent.runtime.as_ref().unwrap().provision_generation,
        Some(2)
    );
    // An unchanged aggregate save/worker claim must preserve additive receipt.
    store
        .claim_sandbox_work(id, "next-worker", NOW.into(), work_lease_deadline())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store.retained_reopen_receipt(id).await.unwrap().as_ref(),
        Some(&receipt)
    );
    assert_eq!(
        second.admit_retained_reopen(request.clone()).await.unwrap(),
        accepted
    );
    let mut different = request.clone();
    different.key_sha256 = "f".repeat(64);
    assert!(matches!(
        store.admit_retained_reopen(different).await,
        Err(SandboxStoreError::Conflict)
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_reopen_mongo_abort_has_no_partial_fences_and_lost_commit_reconciles_exact_state()
{
    let (client, name, store) = database().await;
    let request = reopen_fixture(&store, "sbx_reopen_commit").await;
    let id = &request.snapshot.retained.sandbox.id;
    let mut session = client.start_session().await.unwrap();
    let scope = TransactionScope::start(&mut session).await.unwrap();
    assert!(matches!(
        transaction::apply_scoped(&store, &request, scope)
            .await
            .unwrap(),
        transaction::Applied::Commit { .. }
    ));
    session.abort_transaction().await.unwrap();
    assert_eq!(
        store.retained_reopen_snapshot(id).await.unwrap(),
        request.snapshot
    );
    assert!(store.retained_reopen_receipt(id).await.unwrap().is_none());
    assert_eq!(store.sandbox_events(id, 0, 100).await.unwrap().len(), 0);
    for agent in &request.snapshot.retained.agents {
        let row = store
            .agents
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert!(!row.contains_key("sandbox_recovery_fence"));
    }
    let scope = TransactionScope::start(&mut session).await.unwrap();
    let transaction::Applied::Commit {
        receipt,
        event,
        repeat,
    } = transaction::apply_scoped(&store, &request, scope)
        .await
        .unwrap()
    else {
        panic!("expected write");
    };
    session.commit_transaction().await.unwrap();
    let time = session.operation_time().unwrap();
    // Simulate lost reply, then progress by another worker before reconciliation.
    store
        .claim_sandbox_work(id, "other-worker", NOW.into(), work_lease_deadline())
        .await
        .unwrap()
        .unwrap();
    transaction::reconcile(&store, &receipt, &event, time, repeat)
        .await
        .unwrap();
    let mut wrong = (*receipt).clone();
    wrong.result_sha256 = "0".repeat(64);
    assert!(transaction::reconcile(&store, &wrong, &event, time, repeat)
        .await
        .is_err());
    assert_eq!(
        store.admit_retained_reopen(request).await.unwrap(),
        receipt.accepted
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_reopen_mongo_refuses_durable_changes_after_native_observation() {
    let (client, name, store) = database().await;
    for change in ["parent", "agent", "workspace", "worktree", "log", "remove"] {
        let request = reopen_fixture(&store, &format!("sbx_reopen_{change}")).await;
        let id = &request.snapshot.retained.sandbox.id;
        let agent = &request.snapshot.retained.agents[0];
        match change {
            "parent" => {
                store
                    .sandboxes
                    .update_one(
                        doc! {"_id": id.as_str()},
                        doc! {"$inc": {"revision": 1_i64, "record.revision": 1_i64}},
                    )
                    .await
                    .unwrap();
            }
            "agent" => {
                store
                    .agents
                    .update_one(
                        doc! {"_id": agent.id.as_str()},
                        doc! {"$inc": {"revision": 1_i64, "record.revision": 1_i64}},
                    )
                    .await
                    .unwrap();
            }
            "workspace" => {
                store
                    .workspaces
                    .update_one(
                        doc! {"_id": agent.workspace_id.as_str()},
                        doc! {"$inc": {"revision": 1_i64, "record.revision": 1_i64}},
                    )
                    .await
                    .unwrap();
            }
            "worktree" => {
                store
                    .worktrees
                    .update_one(
                        doc! {"_id": agent.worktree_id.as_str()},
                        doc! {"$inc": {"revision": 1_i64, "record.revision": 1_i64}},
                    )
                    .await
                    .unwrap();
            }
            "log" => {
                store.agent_log.update_one(doc! {"agent_id": agent.id.as_str(), "seq": 3_i64}, doc! {"$set": {"record.appended_at": "2026-09-05T00:31:00Z", "appended_at": "2026-09-05T00:31:00Z"}}).await.unwrap();
            }
            "remove" => {
                store
                    .sandboxes
                    .update_one(
                        doc! {"_id": id.as_str()},
                        doc! {"$set": {"channel_reuse_cleanup_execution": {"fence": 1_i64}}},
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            store.admit_retained_reopen(request.clone()).await.is_err(),
            "{change}"
        );
        assert!(
            store.retained_reopen_receipt(id).await.unwrap().is_none(),
            "{change}"
        );
        assert!(store.sandbox_events(id, 0, 100).await.unwrap().is_empty());
    }
    client.database(&name).drop().await.unwrap();
}
