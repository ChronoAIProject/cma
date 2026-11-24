//! An enabled platform meter requires a valued hold for every model commitment.
use super::super::budget_tests::{configure_metered, open_platform, period_row};
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
    configure_metered(
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
async fn an_unbudgeted_owner_is_refused_even_while_platform_metering_is_open() {
    let (client, name) = database("platform_unbudgeted").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    budget_someone_else(&store).await;
    assert_eq!(
        store
            .reserve_command(request(&agent, &parent, "open"))
            .await,
        Err(CommandLedgerError::Budget(BudgetError::Unavailable))
    );
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
    assert_eq!(
        store
            .reserve_command(request(&agent, &parent, "warned"))
            .await,
        Err(CommandLedgerError::Budget(BudgetError::Unavailable))
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn an_unbudgeted_owners_turns_and_requests_meet_the_stop() {
    let (client, name) = database("platform_unbudgeted_turns").await;
    let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
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
    // The historical bypass remains available while global metering is off.
    assert_eq!(store.reserve_request(&scope, &request).await, Ok(None));
    let queued = record(
        store
            .reserve_command(live_request(&working, &parent, "queued"))
            .await
            .unwrap(),
    );
    budget_someone_else(&store).await;
    crate::sandbox::assert_metered_auxiliary_refusal(
        store.clone(),
        scope.clone(),
        BudgetError::Unavailable,
    )
    .await;
    assert_eq!(
        store.reserve_request(&scope, &request).await,
        Err(BudgetError::Unavailable)
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_reserved_queued_turn_checks_stop_after_its_owner_policy_is_removed() {
    let (client, name) = database("platform_removed_owner").await;
    let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
    configure_metered(&store, &agent.owner).await;
    let queued = record(
        store
            .reserve_command(live_request(&agent, &parent, "reserved-before-stop"))
            .await
            .unwrap(),
    );
    budget_someone_else(&store).await;
    close(&store, &[ClosureReason::SpendStop]).await;
    let worker =
        AgentCommandWorker::new(store.clone(), execution, "removed-owner-worker".into()).unwrap();
    worker
        .run_one(&queued.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(
        sends.load(Ordering::SeqCst),
        0,
        "a previously valued command cannot dispatch under the current STOP"
    );
    let refused = store
        .get_command(&queued.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refused.status, AgentResponseStatus::Failed);
    assert_eq!(
        refused.work.failure.unwrap().code,
        CommandFailureCode::PlatformBudgetExhausted
    );
    client.database(&name).drop().await.unwrap();
}
