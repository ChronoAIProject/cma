use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn suggested_reply_mongo_replicas_admit_one_scope_and_recover_its_original_receipt() {
    let (client, name) = database("suggested_reply").await;
    let (store, mut agent, parent) = fixture(&name, &client).await;
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 2,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Compare PostgreSQL and SQLite.".into(),
        },
        AgentLogKind::AgentMessage {
            content: "The comparison is complete.".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 2 },
    ] {
        store
            .append_agent_log(&agent.id, NOW.into(), kind)
            .await
            .unwrap();
    }
    let revision = agent.revision;
    agent.revision += 1;
    agent.conversation.last_seq = 5;
    store.save_agent(revision, agent.clone()).await.unwrap();
    let conditional = |key| {
        let mut value = request(&agent, &parent, key);
        value.envelope.command = AgentCommand::message("Add criteria.\nKeep limits.");
        AnswerContext {
            turn_sequence: 2,
            answer_sequence: 4,
        }
        .bind(&mut value.envelope.command)
        .unwrap();
        value.envelope.request_fingerprint = value.envelope.command.fingerprint().unwrap();
        value
    };
    let first = conditional("first-tab");
    let other = conditional("other-tab");
    let replica = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (left, right) = tokio::join!(
        store.reserve_command(first.clone()),
        replica.reserve_command(other.clone())
    );
    let (admitted, original) = match (left, right) {
        (Ok(value), Err(CommandLedgerError::NotAdmissible)) => (record(value), first),
        (Err(CommandLedgerError::NotAdmissible), Ok(value)) => (record(value), other),
        other => panic!("expected exactly one eligible answer admission: {other:?}"),
    };
    assert_eq!(store.list_queued_inputs(&agent.id).await.unwrap().len(), 1);
    store
        .reject_unclaimed_command(
            &admitted.receipt.envelope.command_id,
            1,
            CommandFailureCode::AgentUnavailable,
            TWO_MINUTES.into(),
        )
        .await
        .unwrap();
    assert_eq!(
        replica
            .reserve_command(conditional("after-rejection"))
            .await,
        Err(CommandLedgerError::NotAdmissible)
    );
    let recovered = record(replica.reserve_command(original).await.unwrap());
    assert_eq!(recovered.receipt, admitted.receipt);
    client.database(&name).drop().await.unwrap();
}
