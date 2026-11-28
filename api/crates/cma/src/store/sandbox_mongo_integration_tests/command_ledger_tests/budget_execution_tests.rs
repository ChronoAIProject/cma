use super::super::budget_tests::{configure, configure_metered, open_platform};
use super::*;
use layer2_core::budget::{
    BudgetError, BudgetStop, Closure, ClosureReason, OwnerBudget, SpendSource,
};
use layer3_adapter::{ConversationUpdate, ConversationUsage, PromptResponse, TurnStopReason};

async fn read_owner(store: &MongoSandboxStore, agent: &Agent) -> OwnerBudget {
    crate::store::sandbox_mongo::read_record(
        &store
            .model_budget
            .find_one(doc! {"_id": format!("owner:{}", agent.owner.as_str())})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn budget_usage_stop_settles_once_and_top_up_accepts_only_new_input() {
    let (client, name) = database("budget_terminal").await;
    let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
    let policy = configure(&store, &agent.owner).await;
    let original = live_request(&agent, &parent, "original");
    let admitted = record(store.reserve_command(original.clone()).await.unwrap());
    let worker =
        AgentCommandWorker::new(store.clone(), execution.clone(), "budget-worker".into()).unwrap();
    worker
        .run_one(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    let generation = working.conversation.runtime_generation.unwrap();
    let recorder = DurableAgentUpdateRecorder::with_spend_source(
        store.clone(),
        Arc::default(),
        Some(store.clone()),
    );
    let mut assembly = AgentMessageAssembly::default();
    let usage = policy.turn_limit - policy.max_unobserved_units;
    recorder
        .record_update(
            &agent.id,
            generation,
            &mut assembly,
            ConversationUpdate::Usage(ConversationUsage {
                turn_id: "observed-provider-turn".into(),
                input_tokens: usage,
                output_tokens: 0,
                last_input_tokens: usage,
                last_output_tokens: 0,
                context_used: usage,
                context_window: None,
                counters_reset: false,
            }),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap();
    assert_eq!(
        store
            .stop_requested(&agent.id, generation, now)
            .await
            .unwrap()
            .unwrap()
            .reason,
        BudgetError::TurnExhausted
    );
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().unwrap().status,
        AgentStatus::Working
    );
    assert!(read_owner(&store, &agent).await.reserved > 0);
    recorder
        .record_update(
            &agent.id,
            generation,
            &mut assembly,
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::Cancelled)),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let failed = store
        .reconcile_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(failed.status, AgentResponseStatus::Failed);
    let charged = read_owner(&store, &agent).await;
    assert_eq!(charged.reserved, 0);
    assert_eq!(charged.spent, policy.turn_limit);
    let ready = store.get_agent(&agent.id).await.unwrap().unwrap();
    let refused = live_request(&ready, &parent, "refused");
    let reset_at = policy
        .next_reset_at(layer2_core::seconds_from_rfc3339(&refused.admitted_at).unwrap())
        .unwrap();
    assert_eq!(
        store.reserve_command(refused).await,
        Err(CommandLedgerError::OwnerBudgetExhausted { reset_at })
    );
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        record(reopened.reserve_command(original).await.unwrap()),
        failed
    );
    assert_eq!(read_owner(&reopened, &agent).await, charged);
    store.budget_settings.update_one(doc! {"_id": "platform"}, doc! {"$set": {
        "settings.model_budget.owners.0.policy.owner_day_limit": (policy.owner_day_limit * 2) as i64
    }}).await.unwrap();
    let next = record(
        store
            .reserve_command(live_request(&ready, &parent, "explicit-continuation"))
            .await
            .unwrap(),
    );
    worker
        .run_one(&next.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 2);
    let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap();
    assert!(store
        .stop_requested(&agent.id, generation, now)
        .await
        .unwrap()
        .is_none());
    recorder
        .record_update(
            &agent.id,
            generation,
            &mut assembly,
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .reconcile_command(&next.receipt.envelope.command_id)
            .await
            .unwrap()
            .status,
        AgentResponseStatus::Completed
    );
    assert_eq!(
        store
            .get_command(&failed.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentResponseStatus::Failed
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn budget_edit_before_dispatch_refuses_once_with_compatible_durable_precision() {
    let (client, name) = database("budget_queued_edit").await;
    let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
    let policy = configure(&store, &agent.owner).await;
    let original = live_request(&agent, &parent, "queued");
    let admitted = record(store.reserve_command(original.clone()).await.unwrap());
    store.budget_settings.update_one(doc! {"_id": "platform"}, doc! {"$set": {
        "settings.model_budget.owners.0.policy.owner_day_limit": (policy.owner_day_limit / 2) as i64,
        "settings.model_budget.owners.0.policy.turn_limit": (policy.turn_limit / 2) as i64
    }}).await.unwrap();
    AgentCommandWorker::new(store.clone(), execution, "budget-worker".into())
        .unwrap()
        .run_one(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    let id = &admitted.receipt.envelope.command_id;
    let failed = store.get_command(id).await.unwrap().unwrap();
    assert_eq!(failed.status, AgentResponseStatus::Failed);
    assert_eq!(
        failed.work.failure.as_ref().unwrap().code,
        CommandFailureCode::OwnerBudgetExhausted
    );
    assert!(failed.work.dispatch.is_none());
    let owner = read_owner(&store, &agent).await;
    assert_eq!((owner.spent, owner.reserved), (0, 0));
    let raw = store
        .agent_commands
        .find_one(doc! {"_id": id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        raw.get_document("record")
            .unwrap()
            .get_document("work")
            .unwrap()
            .get_document("failure")
            .unwrap()
            .get_str("code")
            .unwrap(),
        "execution_failed"
    );
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        record(reopened.reserve_command(original).await.unwrap()),
        failed
    );
    assert_eq!(reopened.get_command(id).await.unwrap(), Some(failed));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn budget_activation_stop_survives_disable_and_restart_without_a_reservation() {
    let (client, name) = database("budget_activation").await;
    let (store, agent, parent, execution, _, _) = composed(&client, &name).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "already-running"))
            .await
            .unwrap(),
    );
    AgentCommandWorker::new(store.clone(), execution, "budget-worker".into())
        .unwrap()
        .run_one(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    configure(&store, &agent.owner).await;
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    let generation = working.conversation.runtime_generation.unwrap();
    let turn = working.conversation.active_turn.unwrap();
    store
        .model_budget
        .delete_one(doc! {"_id": format!("turn:{}:{turn}", agent.id.as_str())})
        .await
        .unwrap();
    let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap();
    let stop = store
        .stop_requested(&agent.id, generation, now)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stop.reason, BudgetError::Unavailable);
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$unset": {"settings.model_budget": ""}},
        )
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .stop_requested(&agent.id, generation, now)
            .await
            .unwrap(),
        Some(stop)
    );
    assert!(reopened
        .stop_requested(&agent.id, generation + 1, now)
        .await
        .unwrap()
        .is_none());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn lower_usage_timeout_stops_an_already_running_turn() {
    let (client, name) = database("budget_timeout_edit").await;
    let (store, agent, parent, execution, _, _) = composed(&client, &name).await;
    let policy = configure(&store, &agent.owner).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "running"))
            .await
            .unwrap(),
    );
    AgentCommandWorker::new(store.clone(), execution, "budget-worker".into())
        .unwrap()
        .run_one(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    let generation = working.conversation.runtime_generation.unwrap();
    let turn = working.conversation.active_turn.unwrap();
    let raw = store
        .model_budget
        .find_one(doc! {"_id": format!("turn:{}:{turn}", agent.id.as_str())})
        .await
        .unwrap()
        .unwrap();
    let recorded_at = raw
        .get_document("record")
        .unwrap()
        .get_document("reservation")
        .unwrap()
        .get_i64("last_usage_at")
        .unwrap() as u64;
    let now = recorded_at + policy.usage_timeout_seconds;
    assert!(store
        .stop_requested(&agent.id, generation, now)
        .await
        .unwrap()
        .is_none());
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {
                "settings.model_budget.owners.0.policy.usage_timeout_seconds":
                    (policy.usage_timeout_seconds / 2) as i64
            }},
        )
        .await
        .unwrap();
    let stop = store
        .stop_requested(&agent.id, generation, now)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stop.reason, BudgetError::Unavailable);
    assert_eq!(stop.turn_sequence, turn);
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().unwrap().status,
        AgentStatus::Working
    );
    assert!(read_owner(&store, &agent).await.reserved > 0);
    client.database(&name).drop().await.unwrap();
}

async fn running_turn(client: &Client, name: &str) -> (Arc<MongoSandboxStore>, Agent, u64, u64) {
    let (store, agent, parent, execution, _, _) = composed(client, name).await;
    configure_metered(&store, &agent.owner).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "platform-running"))
            .await
            .unwrap(),
    );
    AgentCommandWorker::new(store.clone(), execution, "budget-worker".into())
        .unwrap()
        .run_one(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    let generation = working.conversation.runtime_generation.unwrap();
    let turn = working.conversation.active_turn.unwrap();
    (store, agent, generation, turn)
}

/// Enter a closure the way the guarded outside write does.
async fn enter(store: &MongoSandboxStore, reason: ClosureReason) {
    let (start, _) = open_platform().period_bounds().unwrap();
    let closure = Closure {
        reason,
        since: start,
        settings_revision: 1,
        committed_at_entry: 0,
    };
    store
        .model_budget
        .update_one(
            doc! {"_id": crate::store::budget::period_key(start)},
            doc! {"$push": {"record.closures": mongodb::bson::serialize_to_bson(&closure).unwrap()},
            "$inc": {"record.revision": 1_i64}},
        )
        .await
        .unwrap();
}

fn wall_clock() -> u64 {
    layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn platform_stop_requests_a_guarded_cancel_for_every_active_turn() {
    let (client, name) = database("budget_platform_stop").await;
    let (store, agent, generation, turn) = running_turn(&client, &name).await;
    enter(&store, ClosureReason::SpendStop).await;
    let stop = store
        .stop_requested(&agent.id, generation, wall_clock())
        .await
        .unwrap();
    assert_eq!(
        stop,
        Some(BudgetStop {
            agent_id: agent.id.clone(),
            runtime_generation: generation,
            turn_sequence: turn,
            reason: BudgetError::PlatformExhausted,
        })
    );
    let persisted = store
        .model_budget
        .find_one(doc! {"_id": format!("stop:{}:{turn}", agent.id.as_str())})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::store::sandbox_mongo::read_record::<BudgetError>(&persisted).unwrap(),
        BudgetError::PlatformExhausted
    );
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .stop_requested(&agent.id, generation, wall_clock())
            .await
            .unwrap(),
        stop
    );
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().unwrap().status,
        AgentStatus::Working
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn staleness_alone_does_not_stop_a_running_turn() {
    let (client, name) = database("budget_platform_stale").await;
    let (store, agent, generation, _) = running_turn(&client, &name).await;
    enter(&store, ClosureReason::MeteringUnavailable).await;
    enter(&store, ClosureReason::SpendWarning).await;
    assert_eq!(
        store
            .stop_requested(&agent.id, generation, wall_clock())
            .await
            .unwrap(),
        None
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn stop_reads_are_served_from_the_cache_within_its_ttl() {
    let (client, name) = database("budget_platform_cache").await;
    let (store, agent, generation, _) = running_turn(&client, &name).await;
    let fence = || async {
        store
            .budget_settings
            .find_one(doc! {"_id": "platform"})
            .await
            .unwrap()
            .unwrap()
            .get_i64("model_budget_admission_fence")
            .ok()
    };
    let before = fence().await;
    assert_eq!(
        store
            .stop_requested(&agent.id, generation, wall_clock())
            .await
            .unwrap(),
        None
    );
    enter(&store, ClosureReason::SpendStop).await;
    assert_eq!(
        store
            .stop_requested(&agent.id, generation, wall_clock())
            .await
            .unwrap(),
        None,
        "a cached open read answers until its TTL passes"
    );
    store.age_platform_stop_cache(std::time::Duration::from_secs(5));
    assert_eq!(
        store
            .stop_requested(&agent.id, generation, wall_clock())
            .await
            .unwrap()
            .unwrap()
            .reason,
        BudgetError::PlatformExhausted
    );
    assert_eq!(fence().await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_turn_end_through_the_message_checkpoint_waits_for_local_period_writers() {
    let (client, name) = database("budget_platform_turn_end").await;
    let (store, agent, generation, _) = running_turn(&client, &name).await;
    let held = store.platform.writes.lock().await;
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let agent_id = agent.id.clone();
    let ending = tokio::spawn(async move {
        recorder
            .record_update(
                &agent_id,
                generation,
                &mut AgentMessageAssembly::default(),
                ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
                layer2_core::now_rfc3339(),
            )
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        !ending.is_finished(),
        "a turn end writes the period row and is scheduled behind local period writers"
    );
    drop(held);
    ending.await.unwrap().unwrap();
    let (start, _) = open_platform().period_bounds().unwrap();
    let row = store
        .model_budget
        .find_one(doc! {"_id": crate::store::budget::period_key(start)})
        .await
        .unwrap()
        .unwrap();
    let row: layer2_core::budget::PlatformPeriodLedger =
        crate::store::sandbox_mongo::read_record(&row).unwrap();
    assert_eq!(row.reserved, 0);
    client.database(&name).drop().await.unwrap();
}
