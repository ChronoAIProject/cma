use super::*;
use layer2_core::sandbox::{AgentStatus, FirstMessageRecord, TypedFailure};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn failed_open_replay_survives_restart_and_new_input_is_terminally_refused() {
    let (client, name) = database("command_failed_open").await;
    let (store, mut agent, parent) = fixture(&name, &client).await;
    let revision = agent.revision;
    agent.status = AgentStatus::Starting;
    agent.ready_at = None;
    agent.conversation.conversation_id = None;
    agent.conversation.runtime_generation = None;
    agent.conversation.first_message =
        Some(FirstMessageRecord::new("creation", "Wait for input", NOW).unwrap());
    agent.revision += 1;
    store.save_agent(revision, agent.clone()).await.unwrap();
    let accepted = request(&agent, &parent, "accepted-input");
    let unaccepted = request(&agent, &parent, "never-accepted");
    let original = record(store.reserve_command(accepted.clone()).await.unwrap());
    let revision = agent.revision;
    agent
        .conversation
        .first_message
        .as_mut()
        .unwrap()
        .fail_startup(NOW.into())
        .unwrap();
    agent
        .fail(
            TypedFailure {
                class: "agent_runtime_unsupported".into(),
                detail: "open failed".into(),
            },
            NOW.into(),
        )
        .unwrap();
    store.save_agent(revision, agent.clone()).await.unwrap();
    store
        .reject_unclaimed_command(
            &original.receipt.envelope.command_id,
            original.revision,
            CommandFailureCode::AgentUnavailable,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let terminal = store
        .get_command(&original.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert!(terminal.status.is_terminal());
    assert!(terminal.work.dispatch.is_none());
    drop(store);
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    for _ in 0..2 {
        assert_eq!(
            record(store.reserve_command(accepted.clone()).await.unwrap()),
            terminal
        );
        assert_eq!(
            store.reserve_command(unaccepted.clone()).await,
            Err(CommandLedgerError::NotAdmissible)
        );
    }
    let mut changed = accepted;
    changed.envelope.command = AgentCommand::message("Changed input");
    changed.envelope.request_fingerprint = changed.envelope.command.fingerprint().unwrap();
    assert_eq!(
        store.reserve_command(changed).await,
        Err(CommandLedgerError::IdempotencyConflict)
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}
