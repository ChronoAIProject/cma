use super::*;

#[tokio::test]
async fn principal_scoped_command_replays_original_receipt_across_authorities() {
    let (store, input) = pending().await;
    let creation = created(store.admit_agent(input).await.unwrap());
    let mut first = reservation(&store, &creation.agent, "principal-retry").await;
    first.envelope.authorization.authority_id = "authority-a".into();
    first.envelope.authorization.replay_principal = Some("principal-p".into());
    let CommandReservationResult::Created(original) =
        store.reserve_command(first.clone()).await.unwrap()
    else {
        panic!("expected a new command");
    };
    let mut retry = first;
    retry.envelope.command_id = crate::ids::AgentCommandId::generate();
    retry.response_id = Some(crate::ids::AgentResponseId::generate());
    retry.envelope.authorization.authority_id = "authority-b".into();
    let CommandReservationResult::Existing(replayed) = store.reserve_command(retry).await.unwrap()
    else {
        panic!("expected the original command");
    };
    assert_eq!(replayed, original);
    assert_eq!(
        replayed.receipt.envelope.command_id,
        original.receipt.envelope.command_id
    );
    assert_eq!(replayed.receipt.response_id, original.receipt.response_id);
    let stored = store
        .get_command(&original.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.receipt.envelope.authorization.authority_id,
        "authority-a"
    );
    assert_eq!(
        stored
            .receipt
            .envelope
            .authorization
            .replay_principal
            .as_deref(),
        Some("principal-p")
    );
    assert_eq!(store.lock().agent_commands.len(), 1);
}
