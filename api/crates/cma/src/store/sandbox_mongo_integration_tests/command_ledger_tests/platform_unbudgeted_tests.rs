//! Owners without a model budget policy reserve nothing, yet their new
//! messages, turns and auxiliary requests still meet the platform closures.
use super::super::budget_tests::{configure, open_platform, period_row};
use super::*;
use layer2_core::budget::{
    BudgetError, Closure, ClosureReason, ModelRequest, RequestBudget, RequestScope,
};

async fn close(store: &MongoSandboxStore, reasons: &[ClosureReason]) {
    let (start, _) = open_platform().period_bounds().unwrap();
    let closures: Vec<_> = reasons
        .iter()
        .map(|reason| Closure {
            reason: *reason,
            since: start,
            settings_revision: 1,
            committed_at_entry: 0,
        })
        .collect();
    store
        .model_budget
        .update_one(
            doc! {"_id": crate::store::budget::period_key(start)},
            doc! {"$set": {"record.closures": mongodb::bson::serialize_to_bson(&closures).unwrap()},
            "$inc": {"record.revision": 1_i64}},
        )
        .await
        .unwrap();
}

/// Budgets are enabled for another owner only; the fixture owner has no policy.
async fn budget_someone_else(store: &MongoSandboxStore) {
    configure(
        store,
        &layer2_core::SubjectId("budgeted-someone-else".into()),
    )
    .await;
}

fn reservations() -> Document {
    doc! {"_id": {"$not": {"$regex": "^platform:"}}}
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn an_unbudgeted_owner_is_refused_under_spend_stop_and_reserves_nothing_while_open() {
    let (client, name) = database("platform_unbudgeted").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    budget_someone_else(&store).await;
    assert!(store
        .reserve_command(request(&agent, &parent, "open"))
        .await
        .is_ok());
    assert_eq!(
        store
            .model_budget
            .count_documents(reservations())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        period_row(&store, &open_platform()).await.unwrap().reserved,
        0
    );
    close(&store, &[ClosureReason::SpendStop]).await;
    assert_eq!(
        store
            .reserve_command(request(&agent, &parent, "stopped"))
            .await,
        Err(CommandLedgerError::Budget(BudgetError::PlatformExhausted))
    );
    close(&store, &[ClosureReason::MeteringUnavailable]).await;
    assert_eq!(
        store
            .reserve_command(request(&agent, &parent, "unmetered"))
            .await,
        Err(CommandLedgerError::Budget(BudgetError::Unavailable))
    );
    close(&store, &[ClosureReason::SpendWarning]).await;
    assert!(store
        .reserve_command(request(&agent, &parent, "warned"))
        .await
        .is_ok());
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        2
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn an_unbudgeted_owners_turns_and_requests_meet_the_stop() {
    let (client, name) = database("platform_unbudgeted_turns").await;
    let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
    budget_someone_else(&store).await;
    let running = record(
        store
            .reserve_command(live_request(&agent, &parent, "running"))
            .await
            .unwrap(),
    );
    let worker =
        AgentCommandWorker::new(store.clone(), execution, "platform-worker".into()).unwrap();
    worker
        .run_one(&running.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    let scope = RequestScope {
        agent_id: agent.id.clone(),
        turn_sequence: working.conversation.active_turn.unwrap(),
        request_id: "auxiliary".into(),
    };
    let request = ModelRequest {
        model: "unbudgeted-model".into(),
        input_bytes: 1,
        max_output_tokens: 1,
        fingerprint: "c".repeat(64),
    };
    assert_eq!(store.reserve_request(&scope, &request).await, Ok(None));
    let queued = record(
        store
            .reserve_command(live_request(&working, &parent, "queued"))
            .await
            .unwrap(),
    );
    close(&store, &[ClosureReason::SpendStop]).await;
    let refused = RequestScope {
        request_id: "auxiliary-stopped".into(),
        ..scope
    };
    assert_eq!(
        store.reserve_request(&refused, &request).await,
        Err(BudgetError::PlatformExhausted)
    );
    // Finish the running turn; the queued one must not start under the stop.
    DurableAgentUpdateRecorder::new(store.clone())
        .record_update(
            &agent.id,
            working.conversation.runtime_generation.unwrap(),
            &mut AgentMessageAssembly::default(),
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    worker
        .run_one(&queued.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let failed = store
        .get_command(&queued.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.status, AgentResponseStatus::Failed);
    assert_eq!(
        failed.work.failure.unwrap().code,
        CommandFailureCode::PlatformBudgetExhausted
    );
    assert_eq!(
        store
            .model_budget
            .count_documents(reservations())
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}
