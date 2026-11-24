use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn held_queued_command_timeout_preserves_its_original_receipt() {
    let (client, name) = database("hold_queued_timeout").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "timeout"))
            .await
            .unwrap(),
    );
    let id = &admitted.receipt.envelope.command_id;
    let filter = doc! { "_id": id.as_str() };
    let command = store
        .agent_commands
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    let parent_filter = doc! { "_id": parent.id.as_str() };
    store
        .sandboxes
        .update_one(
            parent_filter.clone(),
            doc! { "$set": { "ledger_maintenance": null } },
        )
        .await
        .unwrap();
    let held_parent = store
        .sandboxes
        .find_one(parent_filter.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store
            .reject_unclaimed_command(
                id,
                admitted.revision,
                CommandFailureCode::QueueTimeout,
                admitted.queue_deadline.clone()
            )
            .await,
        Err(CommandLedgerError::StaleClaim)
    );
    assert_eq!(
        store.agent_commands.find_one(filter).await.unwrap(),
        Some(command)
    );
    assert_eq!(
        store
            .sandboxes
            .find_one(parent_filter.clone())
            .await
            .unwrap(),
        Some(held_parent)
    );
    store
        .sandboxes
        .update_one(
            parent_filter,
            doc! { "$unset": { "ledger_maintenance": "" } },
        )
        .await
        .unwrap();
    store
        .reject_unclaimed_command(
            id,
            admitted.revision,
            CommandFailureCode::QueueTimeout,
            admitted.queue_deadline.clone(),
        )
        .await
        .unwrap();
    let refused = store.get_command(id).await.unwrap().unwrap();
    assert_eq!(refused.status, AgentResponseStatus::Failed);
    assert_eq!(refused.receipt, admitted.receipt);
    assert_eq!(refused.revision, admitted.revision + 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_maintenance_preserves_claimed_command_settlement() {
    let (client, name) = database("hold_command_settlement").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "settlement"))
            .await
            .unwrap(),
    );
    let now = layer2_core::now_rfc3339();
    let held_claim = store
        .claim_next_command(claim_request(&agent, "original", &now))
        .await
        .unwrap()
        .unwrap();
    let claim = &held_claim.work.lease.as_ref().unwrap().claim;
    let command_filter = doc! { "_id": claim.command_id.as_str() };
    let command = store
        .agent_commands
        .find_one(command_filter.clone())
        .await
        .unwrap();
    let lane_filter = doc! { "_id": agent.id.as_str() };
    let lane = store
        .agent_command_orders
        .find_one(lane_filter.clone())
        .await
        .unwrap();
    let replica = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let parents = replica
        .database(&name)
        .collection::<mongodb::bson::Document>("sandboxes");
    let parent_filter = doc! { "_id": parent.id.as_str() };
    for hold in [
        doc! { "phase": "requested" }.into(),
        mongodb::bson::Bson::Null,
    ] {
        parents
            .update_one(
                parent_filter.clone(),
                doc! { "$set": { "ledger_maintenance": hold } },
            )
            .write_concern(mongodb::options::WriteConcern::majority())
            .await
            .unwrap();
        let held_parent = parents.find_one(parent_filter.clone()).await.unwrap();
        assert_eq!(
            store.release_command_claim(claim, now.clone()).await,
            Err(CommandLedgerError::StaleClaim)
        );
        assert_eq!(
            store
                .reject_command(claim, CommandFailureCode::AuthorityRevoked, now.clone())
                .await,
            Err(CommandLedgerError::StaleClaim)
        );
        assert_eq!(
            store
                .agent_commands
                .find_one(command_filter.clone())
                .await
                .unwrap(),
            command
        );
        assert_eq!(
            store
                .agent_command_orders
                .find_one(lane_filter.clone())
                .await
                .unwrap(),
            lane
        );
        assert_eq!(
            parents.find_one(parent_filter.clone()).await.unwrap(),
            held_parent
        );
    }
    parents
        .update_one(
            parent_filter,
            doc! { "$unset": { "ledger_maintenance": "" } },
        )
        .await
        .unwrap();
    store
        .release_command_claim(claim, now.clone())
        .await
        .unwrap();
    let again = store
        .claim_next_command(claim_request(&agent, "successor", &now))
        .await
        .unwrap()
        .unwrap();
    let next = &again.work.lease.as_ref().unwrap().claim;
    assert!(next.fence > claim.fence);
    let rejected = store
        .reject_command(next, CommandFailureCode::AuthorityRevoked, now)
        .await
        .unwrap();
    assert_eq!(rejected.status, AgentResponseStatus::Failed);
    assert_eq!(rejected.receipt, admitted.receipt);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn old_reservation_rows_gain_leases_without_rewriting_acceptance_facts() {
    let (client, name) = database("command_old_row").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "one"))
            .await
            .unwrap(),
    );
    let id = &admitted.receipt.envelope.command_id;
    store
        .agent_commands
        .update_one(
            doc! { "_id": id.as_str() },
            doc! { "$unset": { "record.work": "", "record.target_turn": "" } },
        )
        .await
        .unwrap();
    store
        .agent_command_orders
        .update_one(
            doc! { "_id": agent.id.as_str() },
            doc! { "$unset": { "next_fence": "", "lease": "" } },
        )
        .await
        .unwrap();
    assert_eq!(store.get_command(id).await.unwrap(), Some(admitted.clone()));
    let claimed = store
        .claim_next_command(claim_request(&agent, "worker", &layer2_core::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.receipt, admitted.receipt);
    assert_eq!(claimed.work.lease.unwrap().claim.fence, 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn different_inputs_racing_at_capacity_cannot_consume_two_slots() {
    let (client, name) = database("command_capacity_race").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let replica = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut first = live_request(&agent, &parent, "one");
    first.queue_capacity = 1;
    let mut second = live_request(&agent, &parent, "two");
    second.queue_capacity = 1;
    second.envelope.authorization.authority_id = "another-authority".into();
    let (one, two) = tokio::join!(
        store.reserve_command(first),
        replica.reserve_command(second)
    );
    assert_ne!(one.is_ok(), two.is_ok());
    let error = match (one, two) {
        (Err(error), _) | (_, Err(error)) => error,
        _ => unreachable!(),
    };
    assert_eq!(error, CommandLedgerError::QueueFull);
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );
    assert_eq!(
        store
            .agent_command_orders
            .find_one(doc! { "_id": agent.id.as_str() })
            .await
            .unwrap()
            .unwrap()
            .get_i64("last_order")
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn replicas_recover_one_completion_from_runtime_log_after_reopening() {
    use layer3_adapter::{ConversationUpdate, PromptResponse, TurnStopReason};
    let (client, name) = database("command_recover").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "one"))
            .await
            .unwrap(),
    );
    let claimed = store
        .claim_next_command(claim_request(&agent, "worker", &layer2_core::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    let claim = &claimed.work.lease.as_ref().unwrap().claim;
    store
        .commit_agent_conversation(message_mutation(&agent, &parent, claim))
        .await
        .unwrap();
    let id = &admitted.receipt.envelope.command_id;
    let before = store.reconcile_command(id).await.unwrap();
    assert_eq!(before.status, AgentResponseStatus::InProgress);
    assert_eq!(
        before.work.observation.unwrap().delivery,
        AgentOperationDelivery::DispatchFenced
    );
    layer2_core::sandbox::DurableAgentUpdateRecorder::new(std::sync::Arc::new(store.clone()))
        .record_update(
            &agent.id,
            1,
            &mut layer2_core::sandbox::AgentMessageAssembly::default(),
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    drop(store);
    let one = MongoSandboxStore::open(&client, &name).await.unwrap();
    let two = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (a, b) = tokio::join!(one.reconcile_command(id), two.reconcile_command(id));
    let done = a.unwrap();
    assert_eq!(done, b.unwrap());
    assert_eq!(done.status, AgentResponseStatus::Completed);
    assert_eq!(done.receipt, admitted.receipt);
    assert_eq!(
        done.work.observation.as_ref().unwrap().delivery,
        AgentOperationDelivery::ExecutionObserved
    );
    assert_eq!(done.work.dispatch, before.work.dispatch);
    assert!(done.work.lease.is_none());
    assert!(one
        .claim_next_command(claim_request(
            &agent,
            "replacement",
            &layer2_core::now_rfc3339()
        ))
        .await
        .unwrap()
        .is_none());
    assert_eq!(two.reconcile_command(id).await.unwrap(), done);
    assert_eq!(
        one.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn incomplete_log_refuses_projection_without_clearing_dispatch_custody() {
    let (client, name) = database("command_evidence_gap").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    store
        .reserve_command(live_request(&agent, &parent, "one"))
        .await
        .unwrap();
    let claimed = store
        .claim_next_command(claim_request(&agent, "worker", &layer2_core::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    let claim = &claimed.work.lease.as_ref().unwrap().claim;
    store
        .commit_agent_conversation(message_mutation(&agent, &parent, claim))
        .await
        .unwrap();
    let before = store.get_command(&claim.command_id).await.unwrap();
    let lane = store
        .agent_command_orders
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap();
    store
        .agent_log
        .delete_one(doc! { "agent_id": agent.id.as_str(), "seq": 3_i64 })
        .await
        .unwrap();
    assert_eq!(
        store.reconcile_command(&claim.command_id).await,
        Err(CommandLedgerError::Corrupt)
    );
    assert_eq!(store.get_command(&claim.command_id).await.unwrap(), before);
    assert_eq!(
        store
            .agent_command_orders
            .find_one(doc! { "_id": agent.id.as_str() })
            .await
            .unwrap(),
        lane
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn ambiguous_lease_and_dispatch_commits_recover_without_another_operation() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cma_command_commit_{suffix}");
    let app_name = format!("cma-command-commit-{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let (store, agent, parent) = fixture(&name, &client).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "one"))
            .await
            .unwrap(),
    );
    ambiguous_commits(&admin, &app_name).await;
    let result = store
        .claim_next_command(claim_request(&agent, "worker", &layer2_core::now_rfc3339()))
        .await;
    disable_failpoint(&admin).await;
    assert_eq!(result, Err(CommandLedgerError::Unavailable));
    // A majority read without causal lower bounds may briefly predate the
    // ambiguous commit. Observe its exact identity before inspecting custody.
    let retained = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let row = store
                .get_command(&admitted.receipt.envelope.command_id)
                .await
                .unwrap()
                .unwrap();
            if row.work.lease.is_some() {
                break row;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the injected committed lease becomes majority-visible");
    let lease = retained.work.lease.as_ref().unwrap();
    assert!(retained.work.dispatch.is_none());
    assert!(store
        .claim_next_command(claim_request(
            &agent,
            "replacement",
            &layer2_core::now_rfc3339()
        ))
        .await
        .unwrap()
        .is_none());
    store
        .release_command_claim(&lease.claim, layer2_core::now_rfc3339())
        .await
        .unwrap();
    let claimed = store
        .claim_next_command(claim_request(
            &agent,
            "replacement",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    let claim = &claimed.work.lease.as_ref().unwrap().claim;
    assert!(claim.fence > lease.claim.fence);
    let mutation = message_mutation(&agent, &parent, claim);
    ambiguous_commits(&admin, &app_name).await;
    let result = store.commit_agent_conversation(mutation.clone()).await;
    disable_failpoint(&admin).await;
    let logs = result.unwrap();
    let bound = store.get_command(&claim.command_id).await.unwrap().unwrap();
    assert_eq!(bound.receipt, admitted.receipt);
    assert!(bound.work.dispatch.is_some());
    assert_eq!(logs.len(), 4);
    assert!(matches!(
        store.commit_agent_conversation(mutation).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(store.agent_log(&agent.id, 0, 20).await.unwrap().len(), 5);
    client.database(&name).drop().await.unwrap();
}

async fn ambiguous_commits(admin: &Client, app_name: &str) {
    admin.database("admin").run_command(doc! {
        "configureFailPoint": "failCommand", "mode": { "times": 5_i32 },
        "data": { "failCommands": ["commitTransaction"], "appName": app_name,
            "writeConcernError": { "code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected ambiguous command commit" } },
    }).await.unwrap();
}

async fn disable_failpoint(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .expect("disable failpoint before assertions");
}
