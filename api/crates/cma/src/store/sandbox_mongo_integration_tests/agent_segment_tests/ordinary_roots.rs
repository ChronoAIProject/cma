//! Real adoption, due discovery, worker claim/publication and admission roots.
use super::retained_assessment::fixture;
use super::worker_discovery::finish_legacy_connection;
use super::*;
use crate::store::sandbox_mongo::{commit_transaction, CommitOutcome};
use layer2_core::sandbox::agent_io::{AgentCommandStore, CommandLedgerError};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn adopted_discovery_claim_publication_and_command_admission_keep_v2() {
    let (client, name, store, agent) = fixture("ordinary_v2").await;
    let due = store.due_agent_work(NOW, 20).await.unwrap();
    assert!(due.contains(&agent.id));
    let ready = finish_legacy_connection(&store, agent.clone()).await;
    assert_eq!(ready.conversation.last_seq, agent.conversation.last_seq + 1);
    let parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let request = super::super::command_ledger_tests::request(&ready, &parent, "ordinary-v2");
    let accepted = store.reserve_command(request.clone()).await.unwrap();
    assert_eq!(
        record(store.reserve_command(request).await.unwrap()).receipt,
        record(accepted).receipt
    );
    let row = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("document_schema_version").unwrap(), 2);
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! {"agent_id":agent.id.as_str()})
            .await
            .unwrap(),
        ready.conversation.last_seq
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn adopted_admission_abort_and_foreign_proof_leave_no_effects() {
    let (client, name, store, agent) = fixture("ordinary_v2_abort").await;
    let ready = finish_legacy_connection(&store, agent).await;
    let parent = store.get_sandbox(&ready.sandbox_id).await.unwrap().unwrap();
    let request = super::super::command_ledger_tests::request(&ready, &parent, "ordinary-v2-abort");
    let before = store
        .agents
        .find_one(doc! {"_id":ready.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    assert!(
        crate::store::agent_commands::reserve::apply(&store, &request, &mut session)
            .await
            .is_ok()
    );
    session.abort_transaction().await.unwrap();
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id":ready.id.as_str()})
            .await
            .unwrap(),
        Some(before.clone())
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    store
        .agents
        .update_one(
            doc! {"_id":ready.id.as_str()},
            doc! {"$inc":{"agent_log_head_revision":1_i64}},
        )
        .await
        .unwrap();
    assert_eq!(
        store.reserve_command(request.clone()).await.unwrap_err(),
        CommandLedgerError::Corrupt
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    store
        .agents
        .replace_one(doc! {"_id":ready.id.as_str()}, before)
        .await
        .unwrap();
    start_transaction(&mut session).await.unwrap();
    assert!(
        crate::store::agent_commands::reserve::apply(&store, &request, &mut session)
            .await
            .is_ok()
    );
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    client.database(&name).drop().await.unwrap();
}

fn record(
    result: layer2_core::sandbox::agent_io::CommandReservationResult,
) -> layer2_core::sandbox::agent_io::AdmittedAgentCommand {
    use layer2_core::sandbox::agent_io::CommandReservationResult;
    match result {
        CommandReservationResult::Created(record) | CommandReservationResult::Existing(record) => {
            *record
        }
    }
}
