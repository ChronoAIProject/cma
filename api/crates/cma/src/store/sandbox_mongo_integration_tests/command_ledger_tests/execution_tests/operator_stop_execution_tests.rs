//! A stop affects new admission, never already accepted turns or settlement.
use super::*;
use layer2_core::admission::AdmissionControlStore;
use layer2_core::budget::{ModelRequest, RequestBudget, RequestScope, RequestUsage, SpendSource};

async fn stop(client: &Client, name: &str) {
    crate::store::admission_control::MongoAdmissionControlStore::open(client, name)
        .await
        .unwrap()
        .change(
            0,
            &SubjectId::new("operator"),
            "contain new work",
            true,
            100,
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operator_stop_allows_queued_dispatch_and_final_settlement_once() {
    let (client, name) = database("operator_running").await;
    let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
    super::super::budget_tests::configure_metered(&store, &agent.owner).await;
    // Leave room for another command so its refusal proves operator control,
    // rather than the spend refusal that intentionally has precedence.
    super::super::budget_tests::raise_owner_limit(&store, 2).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "before-stop"))
            .await
            .unwrap(),
    );
    stop(&client, &name).await;
    let worker =
        AgentCommandWorker::new(store.clone(), execution, "continued-worker".into()).unwrap();
    let id = &admitted.receipt.envelope.command_id;
    worker.run_one(id).await.unwrap();
    worker.run_one(id).await.unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    let generation = working.conversation.runtime_generation.unwrap();
    assert!(working.conversation.active_turn.is_some());
    assert_eq!(
        store
            .reserve_command(live_request(&working, &parent, "after-stop"))
            .await
            .unwrap_err(),
        CommandLedgerError::AdmissionPaused
    );
    let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap();
    assert!(store
        .stop_requested(&agent.id, generation, now)
        .await
        .unwrap()
        .is_none());
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let mut assembly = AgentMessageAssembly::default();
    recorder
        .record_update(
            &agent.id,
            generation,
            &mut assembly,
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let completed = store.reconcile_command(id).await.unwrap();
    assert_eq!(completed.status, AgentResponseStatus::Completed);
    assert_eq!(store.reconcile_command(id).await.unwrap(), completed);
    worker.run_one(id).await.unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let owner = store
        .model_budget
        .find_one(doc! {"_id": format!("owner:{}", agent.owner.as_str())})
        .await
        .unwrap()
        .unwrap();
    let owner: layer2_core::budget::OwnerBudget =
        crate::store::sandbox_mongo::read_record(&owner).unwrap();
    assert_eq!(owner.reserved, 0);
    let period = super::super::budget_tests::period_row(
        &store,
        &super::super::budget_tests::open_platform(),
    )
    .await
    .unwrap();
    assert_eq!(period.reserved, 0);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operator_stop_allows_an_unbudgeted_running_turns_auxiliary_request() {
    let (client, name) = database("operator_unbudgeted_aux").await;
    let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "before-stop"))
            .await
            .unwrap(),
    );
    stop(&client, &name).await;
    AgentCommandWorker::new(store.clone(), execution, "continued-worker".into())
        .unwrap()
        .run_one(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    let scope = RequestScope {
        agent_id: agent.id,
        turn_sequence: working.conversation.active_turn.unwrap(),
        request_id: "continued-aux".into(),
    };
    let request = ModelRequest {
        model: "unconfigured-model".into(),
        input_bytes: 1,
        max_output_tokens: 1,
        fingerprint: "c".repeat(64),
    };
    assert_eq!(store.reserve_request(&scope, &request).await, Ok(None));
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operator_stop_allows_priced_auxiliary_work_with_the_existing_finality_proof() {
    let (client, name) = database("operator_budgeted_aux").await;
    // Reuse #1741's authoritative-settlement fixture. A runtime terminal event
    // alone cannot authorize another budgeted request under its spend contract.
    let (store, scope, request) = super::request_budget::completed(&client, &name).await;
    super::super::budget_tests::meter(&store).await;
    stop(&client, &name).await;
    let hold = store
        .reserve_request(&scope, &request)
        .await
        .unwrap()
        .unwrap();
    let usage = RequestUsage {
        input_tokens: 1,
        output_tokens: 1,
    };
    store.settle_request(&hold, usage).await.unwrap();
    store.settle_request(&hold, usage).await.unwrap();
    assert_eq!(
        store
            .model_budget
            .count_documents(doc! {"_id": {"$regex": "^request:"}})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn review_round_one_unreadable_control_refuses_fresh_work_but_preserves_turn_convergence() {
    use layer2_core::admission::{AdmissionCause, AdmissionRefusal};
    use layer2_core::budget::{BudgetError, Closure, ClosureReason, PlatformSpendLedger};
    for unreadable in [
        mongodb::bson::Bson::Null,
        mongodb::bson::bson!({"revision": "unsupported"}),
        mongodb::bson::bson!({"revision": 0_i64, "operator_stop": null, "last_change": null, "future_field": true}),
    ] {
        let (client, name) = database("unreadable_operator_control").await;
        let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
        super::super::budget_tests::configure_metered(&store, &agent.owner).await;
        super::super::budget_tests::raise_owner_limit(&store, 2).await;
        let admitted = record(
            store
                .reserve_command(live_request(&agent, &parent, "before-corruption"))
                .await
                .unwrap(),
        );
        store
            .budget_settings
            .update_one(
                doc! {"_id": "platform"},
                doc! {"$set": {"admission_control": unreadable}},
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .reserve_command(live_request(&agent, &parent, "fresh-corrupt"))
                .await
                .unwrap_err(),
            CommandLedgerError::AdmissionPaused
        );
        let state = store.admission_state(100).await.unwrap();
        assert_eq!(state.commitment_refusal(), Some(AdmissionRefusal::Paused));
        assert_eq!(
            state.commitment_refusal_for(AdmissionCause::Continuation),
            None
        );
        assert!(state.closed_for_registration());
        let worker =
            AgentCommandWorker::new(store.clone(), execution, "continued-worker".into()).unwrap();
        let id = &admitted.receipt.envelope.command_id;
        worker.run_one(id).await.unwrap();
        worker.run_one(id).await.unwrap();
        assert_eq!(sends.load(Ordering::SeqCst), 1);
        let working = store.get_agent(&agent.id).await.unwrap().unwrap();
        let generation = working.conversation.runtime_generation.unwrap();
        let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap();
        assert!(store
            .stop_requested(&agent.id, generation, now)
            .await
            .unwrap()
            .is_none());
        assert!(store.spend_state(now).await.unwrap().metering_enabled);
        assert!(store.evaluate_period(now).await.unwrap().is_some());
        let start = super::super::budget_tests::open_platform()
            .period_bounds()
            .unwrap()
            .0;
        let closure = Closure {
            reason: ClosureReason::SpendStop,
            since: now,
            settings_revision: 1,
            committed_at_entry: 0,
        };
        store.model_budget.update_one(doc! {"_id": crate::store::budget::period_key(start)},
            doc! {"$push": {"record.closures": mongodb::bson::serialize_to_bson(&closure).unwrap()}}).await.unwrap();
        // Respect the existing spend poll TTL before observing a later closure.
        store.age_platform_stop_cache(std::time::Duration::from_secs(6));
        assert_eq!(
            store
                .stop_requested(&agent.id, generation, now)
                .await
                .unwrap()
                .unwrap()
                .reason,
            BudgetError::PlatformExhausted
        );
        let recorder = DurableAgentUpdateRecorder::new(store.clone());
        let mut assembly = AgentMessageAssembly::default();
        recorder
            .record_update(
                &agent.id,
                generation,
                &mut assembly,
                layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                    layer3_adapter::TurnStopReason::EndTurn,
                )),
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
        let completed = store.reconcile_command(id).await.unwrap();
        assert_eq!(completed.status, AgentResponseStatus::Completed);
        assert_eq!(store.reconcile_command(id).await.unwrap(), completed);
        let period = super::super::budget_tests::period_row(
            &store,
            &super::super::budget_tests::open_platform(),
        )
        .await
        .unwrap();
        assert_eq!(period.reserved, 0);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unreadable_control_preserves_priced_auxiliary_work_and_cloud_observation() {
    use layer2_core::budget::{CloudObservationInput, PlatformSpendLedger};
    let (client, name) = database("unreadable_operator_aux").await;
    let (store, scope, request) = super::request_budget::completed(&client, &name).await;
    super::super::budget_tests::meter(&store).await;
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"admission_control": null}},
        )
        .await
        .unwrap();
    let hold = store
        .reserve_request(&scope, &request)
        .await
        .unwrap()
        .unwrap();
    let usage = RequestUsage {
        input_tokens: 1,
        output_tokens: 1,
    };
    store.settle_request(&hold, usage).await.unwrap();
    store.settle_request(&hold, usage).await.unwrap();
    let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap();
    let state = store
        .record_cloud_observation(
            CloudObservationInput {
                observed_through: now,
                units: 0,
            },
            now,
        )
        .await
        .unwrap();
    assert!(state.metering_enabled);
    assert_eq!(state.ledger.unwrap().cloud.unwrap().observed_through, now);
    client.database(&name).drop().await.unwrap();
}
