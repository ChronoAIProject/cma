//! Actual command writers preserve finality across executable boundaries.
use super::*;
use crate::store::writer_fixture::{client, database_name, emit, hash, input};
use serde_json::{json, Value};

async fn snapshot(store: &MongoSandboxStore, id: &AgentCommandId) -> Value {
    let row = store
        .agent_commands
        .find_one(doc! {"_id":id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let command = store.get_command(id).await.unwrap().unwrap();
    json!({"id_hash":hash(id),"receipt_hash":hash(&command.receipt),"status":command.status,
        "failure":command.work.failure.as_ref().map(|f|f.code),"marker":row.get("admission_capacity_failure"),
        "revision":command.revision,"dispatch":command.work.dispatch.is_some(),"row_hash":hash(&row)})
}

#[tokio::test]
#[ignore = "requires isolated cross-version writer fixture"]
async fn command_round() {
    let client = client().await;
    let database = database_name();
    let action = input("CMA_WRITER_ACTION");
    if action == "seed" {
        let (store, agent, parent) = fixture(&database, &client).await;
        let admitted = record(
            store
                .reserve_command(request(&agent, &parent, "writer-command"))
                .await
                .unwrap(),
        );
        emit(json!({"after":snapshot(&store,&admitted.receipt.envelope.command_id).await}));
        return;
    }
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let row = store
        .agent_commands
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    let id = AgentCommandId(row.get_str("_id").unwrap().into());
    let before = snapshot(&store, &id).await;
    if action == "reject" {
        let command = store.get_command(&id).await.unwrap().unwrap();
        let code: CommandFailureCode =
            serde_json::from_value(json!(input("CMA_WRITER_CODE"))).unwrap();
        store
            .reject_unclaimed_command(&id, command.revision, code, NOW.into())
            .await
            .unwrap();
        assert!(store
            .reject_unclaimed_command(&id, command.revision, code, NOW.into())
            .await
            .is_err());
    } else if action == "terminal_write" {
        let command = store.get_command(&id).await.unwrap().unwrap();
        assert_eq!(command.status, AgentResponseStatus::Failed);
        assert!(store
            .reject_unclaimed_command(
                &id,
                command.revision,
                CommandFailureCode::ExecutionFailed,
                NOW.into()
            )
            .await
            .is_err());
        assert_eq!(snapshot(&store, &id).await, before);
    } else {
        assert_eq!(action, "read");
    }
    let after = snapshot(&store, &id).await;
    assert_eq!(before["id_hash"], after["id_hash"]);
    assert_eq!(before["receipt_hash"], after["receipt_hash"]);
    assert_eq!(after["dispatch"], false);
    emit(json!({"before":before,"after":after}));
}
