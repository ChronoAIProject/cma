//! Capacity loss at turn start follows the existing terminal refusal path.
use super::super::ceilings_tests::configure;
use super::*;
use layer2_core::admission::AdmissionCeilings;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn lowered_capacity_at_start_is_terminal_typed_and_never_replayed() {
    let (client, name) = database("session_start_terminal").await;
    let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
    let request = live_request(&agent, &parent, "admitted-before-lowering");
    let admitted = record(store.reserve_command(request.clone()).await.unwrap());
    configure(
        &store,
        AdmissionCeilings {
            active_sessions: Some(0),
            ..Default::default()
        },
    )
    .await;
    let worker =
        AgentCommandWorker::new(store.clone(), execution, "capacity-worker".into()).unwrap();
    let id = &admitted.receipt.envelope.command_id;
    worker.run_one(id).await.unwrap();
    worker.run_one(id).await.unwrap();
    let failed = store.get_command(id).await.unwrap().unwrap();
    assert_eq!(failed.status, AgentResponseStatus::Failed);
    assert_eq!(
        failed.work.failure.as_ref().unwrap().code,
        CommandFailureCode::SessionCapacityUnavailable
    );
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().unwrap().status,
        AgentStatus::Ready
    );
    let row = store
        .agent_commands
        .find_one(doc! {"_id": id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.get_str("admission_capacity_failure").unwrap(),
        "session_capacity_unavailable"
    );
    assert_eq!(
        row.get_document("record")
            .unwrap()
            .get_document("work")
            .unwrap()
            .get_document("failure")
            .unwrap()
            .get_str("code")
            .unwrap(),
        "execution_failed"
    );
    configure(&store, AdmissionCeilings::default()).await;
    assert_eq!(
        record(store.reserve_command(request).await.unwrap()),
        failed
    );
    worker.run_one(id).await.unwrap();
    assert_eq!(store.get_command(id).await.unwrap().unwrap(), failed);
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn messages_to_an_active_agent_consume_no_capacity_and_unreadable_state_keeps_it_running() {
    let (client, name) = database("session_active_queue").await;
    let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "first"))
            .await
            .unwrap(),
    );
    let worker = AgentCommandWorker::new(store.clone(), execution, "active-worker".into()).unwrap();
    let id = &admitted.receipt.envelope.command_id;
    worker.run_one(id).await.unwrap();
    worker.run_one(id).await.unwrap();
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(working.status, AgentStatus::Working);
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    configure(
        &store,
        AdmissionCeilings {
            active_sessions: Some(0),
            owner_active_sessions: Some(0),
            ..Default::default()
        },
    )
    .await;
    store
        .reserve_command(live_request(&working, &parent, "queued-at-zero"))
        .await
        .unwrap();
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"admission_ceilings": null}},
        )
        .await
        .unwrap();
    store
        .reserve_command(live_request(&working, &parent, "queued-while-unreadable"))
        .await
        .unwrap();
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(working));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unreadable_ceilings_never_block_starting_an_already_admitted_message() {
    let (client, name) = database("session_corrupt_continuation").await;
    let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "accepted"))
            .await
            .unwrap(),
    );
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"admission_ceilings": null}},
        )
        .await
        .unwrap();
    let worker =
        AgentCommandWorker::new(store.clone(), execution, "continued-worker".into()).unwrap();
    let id = &admitted.receipt.envelope.command_id;
    worker.run_one(id).await.unwrap();
    worker.run_one(id).await.unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().unwrap().status,
        AgentStatus::Working
    );
    client.database(&name).drop().await.unwrap();
}
