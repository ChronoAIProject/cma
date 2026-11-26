use super::*;
use layer3_adapter::{ConversationUpdate, PromptResponse, TurnStopReason};
use mongodb::bson::{Bson, Document};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn held_sandbox_preserves_unprojected_command_completion() {
    let (client, name) = database("hold_command_reconciliation").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "completion"))
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
    let original = store.reconcile_command(id).await.unwrap();
    assert_eq!(original.status, AgentResponseStatus::InProgress);
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
    let replica = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let parents = replica.database(&name).collection::<Document>("sandboxes");
    let parent_filter = doc! { "_id": parent.id.as_str() };
    let agent_filter = doc! { "_id": agent.id.as_str() };
    let command_filter = doc! { "_id": id.as_str() };
    let stored_agent = store
        .agents
        .find_one(agent_filter.clone())
        .await
        .unwrap()
        .unwrap();
    let stored_lane = store
        .agent_command_orders
        .find_one(agent_filter.clone())
        .await
        .unwrap()
        .unwrap();
    let stored_command = store
        .agent_commands
        .find_one(command_filter.clone())
        .await
        .unwrap()
        .unwrap();
    for hold in [doc! { "phase": "requested" }.into(), Bson::Null] {
        parents
            .update_one(
                parent_filter.clone(),
                doc! { "$set": { "ledger_maintenance": hold } },
            )
            .write_concern(mongodb::options::WriteConcern::majority())
            .await
            .unwrap();
        let held_parent = parents
            .find_one(parent_filter.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            store.reconcile_command(id).await,
            Err(CommandLedgerError::Conflict)
        );
        assert_eq!(
            store
                .agents
                .find_one(agent_filter.clone())
                .await
                .unwrap()
                .as_ref(),
            Some(&stored_agent)
        );
        assert_eq!(
            store
                .agent_command_orders
                .find_one(agent_filter.clone())
                .await
                .unwrap()
                .as_ref(),
            Some(&stored_lane)
        );
        assert_eq!(
            store
                .agent_commands
                .find_one(command_filter.clone())
                .await
                .unwrap()
                .as_ref(),
            Some(&stored_command)
        );
        assert_eq!(
            parents.find_one(parent_filter.clone()).await.unwrap(),
            Some(held_parent)
        );
    }
    parents
        .update_one(
            parent_filter.clone(),
            doc! { "$unset": { "ledger_maintenance": "" } },
        )
        .await
        .unwrap();
    let completed = store.reconcile_command(id).await.unwrap();
    assert_eq!(completed.status, AgentResponseStatus::Completed);
    assert_eq!(completed.receipt, original.receipt);
    let parent_before_terminal_read = parents.find_one(parent_filter.clone()).await.unwrap();
    assert_eq!(store.reconcile_command(id).await.unwrap(), completed);
    assert_eq!(
        parents.find_one(parent_filter).await.unwrap(),
        parent_before_terminal_read
    );
    client.database(&name).drop().await.unwrap();
}
