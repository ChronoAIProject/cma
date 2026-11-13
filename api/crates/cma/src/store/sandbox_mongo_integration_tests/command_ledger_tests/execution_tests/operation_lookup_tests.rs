use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operation_command_lookup_is_exact_and_rejects_duplicate_bindings() {
    let (client, name) = database("operation_command_lookup").await;
    let (store, agent, parent, execution, _, _) = composed(&client, &name).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "one"))
            .await
            .unwrap(),
    );
    let id = &admitted.receipt.envelope.command_id;
    AgentCommandWorker::new(store.clone(), execution, "lookup-worker".into())
        .unwrap()
        .run_one(id)
        .await
        .unwrap();
    let current = store.get_command(id).await.unwrap().unwrap();
    let operation = current.work.dispatch.as_ref().unwrap().operation_sequence;
    assert_eq!(
        store
            .get_operation_command(&agent.id, operation)
            .await
            .unwrap(),
        Some(current.clone())
    );
    assert!(store
        .get_operation_command(&AgentId("foreign-agent".into()), operation)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .get_operation_command(&agent.id, operation + 1)
        .await
        .unwrap()
        .is_none());
    let mut duplicate = current;
    duplicate.receipt.envelope.command_id = AgentCommandId::generate();
    duplicate.work.dispatch.as_mut().unwrap().claim.command_id =
        duplicate.receipt.envelope.command_id.clone();
    duplicate.receipt.response_id = Some(AgentResponseId::generate());
    duplicate.receipt.agent_order += 1;
    duplicate.key_sha256 = command_key_digest("duplicate-operation").unwrap();
    let document = crate::store::agent_commands::command_document(&duplicate).unwrap();
    store.agent_commands.insert_one(document).await.unwrap();
    assert_eq!(
        store.get_operation_command(&agent.id, operation).await,
        Err(CommandLedgerError::Corrupt)
    );
    client.database(&name).drop().await.unwrap();
}
