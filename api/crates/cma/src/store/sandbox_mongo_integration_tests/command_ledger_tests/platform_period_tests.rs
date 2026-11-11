//! The platform period row moves inside the owner reservation transactions.
use super::super::budget_tests::{
    add_owner, budget_capabilities, configure, configure_with_platform, observe, open_platform,
    period_row, raise_owner_limit,
};
use super::*;
use layer2_core::budget::{BudgetError, ClosureReason, PlatformBudgetPolicy};
use layer3_adapter::{ConversationUpdate, PromptResponse, TurnStopReason};

fn seconds(stamp: &str) -> u64 {
    layer2_core::seconds_from_rfc3339(stamp).unwrap()
}

/// A long period whose disposable level is exactly `cap` (no reserves).
fn capped(cap: u64) -> PlatformBudgetPolicy {
    PlatformBudgetPolicy {
        period_cap: cap,
        ..open_platform()
    }
}

/// Configure `cap` with a zero observation at the fixture clock, so command
/// admissions at `NOW` see no projected cloud spend.
async fn configure_capped(store: &MongoSandboxStore, agent: &Agent, cap: u64) -> u64 {
    let platform = capped(cap);
    let policy = configure_with_platform(store, &agent.owner, platform.clone(), 1).await;
    observe(store, &platform, 0, seconds(NOW)).await;
    raise_owner_limit(store, 64).await;
    policy.turn_limit
}

fn reasons(platform: &layer2_core::budget::PlatformPeriodLedger) -> Vec<ClosureReason> {
    platform
        .closures
        .iter()
        .map(|closure| closure.reason)
        .collect()
}

async fn finish_turn(store: &Arc<MongoSandboxStore>, agent: &Agent) {
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    DurableAgentUpdateRecorder::new(store.clone())
        .record_update(
            &agent.id,
            working.conversation.runtime_generation.unwrap(),
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
}

async fn start_turn(
    store: &Arc<MongoSandboxStore>,
    execution: Arc<crate::commands::ResourceCommandExecution>,
    admitted: &AdmittedAgentCommand,
) {
    AgentCommandWorker::new(store.clone(), execution, "platform-worker".into())
        .unwrap()
        .run_one(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn turn_reservation_mirrors_into_the_period_row_in_the_same_transaction() {
    let (client, name) = database("platform_turn_mirror").await;
    let (store, agent, parent, execution, _, _) = composed(&client, &name).await;
    let policy = configure(&store, &agent.owner).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "mirrored"))
            .await
            .unwrap(),
    );
    let row = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!((row.reserved, row.spent), (policy.turn_limit, 0));
    start_turn(&store, execution, &admitted).await;
    let row = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!((row.reserved, row.spent), (policy.turn_limit, 0));
    finish_turn(&store, &agent).await;
    let row = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!((row.reserved, row.spent), (0, policy.turn_limit));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn request_hold_mirrors_and_settles_against_the_period_row() {
    use layer2_core::budget::{RequestBudget, RequestUsage};
    let (client, name) = database("platform_request_mirror").await;
    let (store, scope, request) = super::request_budget::completed(&client, &name).await;
    let before = period_row(&store, &open_platform()).await.unwrap();
    let hold = store
        .reserve_request(&scope, &request)
        .await
        .unwrap()
        .unwrap();
    let held = period_row(&store, &open_platform()).await.unwrap();
    let liability = held.reserved - before.reserved;
    assert!(liability > 0);
    assert_eq!(held.spent, before.spent);
    let usage = RequestUsage {
        input_tokens: 1,
        output_tokens: 1,
    };
    store.settle_request(&hold, usage).await.unwrap();
    store.settle_request(&hold, usage).await.unwrap();
    let settled = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!(settled.reserved, before.reserved);
    assert_eq!(settled.spent, before.spent + 2);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn proven_non_dispatch_releases_period_liability() {
    let (client, name) = database("platform_release").await;
    let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
    let policy = configure(&store, &agent.owner).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "never-dispatched"))
            .await
            .unwrap(),
    );
    assert_eq!(
        period_row(&store, &open_platform()).await.unwrap().reserved,
        policy.turn_limit
    );
    store.budget_settings.update_one(doc! {"_id": "platform"}, doc! {"$set": {
        "settings.model_budget.owners.0.policy.turn_limit": (policy.turn_limit / 2) as i64,
        "settings.model_budget.owners.0.policy.owner_day_limit": (policy.owner_day_limit / 2) as i64,
    }}).await.unwrap();
    start_turn(&store, execution, &admitted).await;
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    let failed = store
        .get_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.status, AgentResponseStatus::Failed);
    assert!(failed.work.dispatch.is_none());
    let row = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!((row.reserved, row.spent), (0, 0));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn admission_refuses_with_platform_exhausted_and_records_the_closure() {
    let (client, name) = database("platform_refusal").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let turn_limit = configure_capped(&store, &agent, u64::from(u8::MAX) + 1).await;
    assert!(store
        .reserve_command(request(&agent, &parent, "fits"))
        .await
        .is_ok());
    assert_eq!(
        store
            .reserve_command(request(&agent, &parent, "overshoots"))
            .await,
        Err(CommandLedgerError::Budget(BudgetError::PlatformExhausted))
    );
    let row = period_row(&store, &capped(turn_limit + 1)).await.unwrap();
    assert_eq!(row.reserved, turn_limit);
    assert_eq!(
        reasons(&row),
        vec![ClosureReason::SpendWarning, ClosureReason::SpendStop]
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_refused_admission_leaves_one_spend_stop_visible_after_its_transaction_aborted() {
    let (client, name) = database("platform_refusal_visible").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    // Nothing fits: the very first liability is refused on an untouched row.
    configure_capped(&store, &agent, u64::from(u8::MAX) - 1).await;
    for key in ["refused-first", "refused-again"] {
        assert_eq!(
            store.reserve_command(request(&agent, &parent, key)).await,
            Err(CommandLedgerError::Budget(BudgetError::PlatformExhausted))
        );
    }
    let reader = MongoSandboxStore::open(&client, &name).await.unwrap();
    let row = period_row(&reader, &capped(u64::from(u8::MAX) - 1))
        .await
        .unwrap();
    assert_eq!(row.reserved, 0);
    assert_eq!(
        reasons(&row)
            .iter()
            .filter(|reason| **reason == ClosureReason::SpendStop)
            .count(),
        1
    );
    let owner = reader
        .model_budget
        .find_one(doc! {"_id": format!("owner:{}", agent.owner.as_str())})
        .await
        .unwrap();
    assert!(
        owner.is_none(),
        "the refused transactions left no owner row"
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn two_stores_crossing_the_stop_level_record_one_closure() {
    let (client, name) = database("platform_two_stores").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let turn_limit = configure_capped(&store, &agent, u64::from(u8::MAX) + 1).await;
    let mut other_parent = active_sandbox("sbx_platform_other");
    other_parent.owner = layer2_core::SubjectId("platform-other-owner".into());
    store.create_sandbox(other_parent.clone()).await.unwrap();
    let mut other = ready_agent(&other_parent, "agt_platform_other");
    other.capabilities = budget_capabilities();
    insert_agent(&store, &other, NOW).await;
    add_owner(&store, &other.owner).await;
    let peer = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (first, second) = tokio::join!(
        store.reserve_command(request(&agent, &parent, "first")),
        peer.reserve_command(request(&other, &other_parent, "second")),
    );
    assert_ne!(first.is_ok(), second.is_ok());
    assert_eq!(
        first.err().or(second.err()).unwrap(),
        CommandLedgerError::Budget(BudgetError::PlatformExhausted)
    );
    let row = period_row(&store, &capped(turn_limit + 1)).await.unwrap();
    assert_eq!(row.reserved, turn_limit);
    assert_eq!(row.closures.len(), 2);
    assert_eq!(
        reasons(&row)
            .iter()
            .filter(|reason| **reason == ClosureReason::SpendStop)
            .count(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn twenty_concurrent_admissions_on_one_store_exhaust_no_retry_budget() {
    let (client, name) = database("platform_twenty").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    configure(&store, &agent.owner).await;
    let mut requests = Vec::new();
    for index in 0..20 {
        let mut sandbox = active_sandbox(&format!("sbx_platform_{index}"));
        sandbox.owner = layer2_core::SubjectId(format!("platform-owner-{index}"));
        store.create_sandbox(sandbox.clone()).await.unwrap();
        let mut owned = ready_agent(&sandbox, &format!("agt_platform_{index}"));
        owned.capabilities = budget_capabilities();
        insert_agent(&store, &owned, NOW).await;
        add_owner(&store, &owned.owner).await;
        requests.push(request(&owned, &sandbox, &format!("concurrent-{index}")));
    }
    drop((agent, parent));
    let results = futures_util::future::join_all(
        requests
            .into_iter()
            .map(|request| store.reserve_command(request)),
    )
    .await;
    // Retry exhaustion surfaces as Conflict and accounting faults as Unavailable.
    for result in results {
        assert!(result.is_ok(), "{result:?}");
    }
    let row = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!(row.reserved, 20 * u64::from(u8::MAX));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn zero_liability_admission_never_writes_the_settings_fence() {
    let (client, name) = database("platform_zero_liability").await;
    let (store, agent, _) = fixture(&name, &client).await;
    configure(&store, &agent.owner).await;
    let fence = |document: Document| document.get_i64("model_budget_admission_fence").ok();
    let settings = || async {
        store
            .budget_settings
            .find_one(doc! {"_id": "platform"})
            .await
            .unwrap()
            .unwrap()
    };
    let before = fence(settings().await);
    let row = period_row(&store, &open_platform()).await.unwrap();
    let mut session = client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    let opened = crate::store::budget::require_open_with(
        &store.budget_settings,
        &store.model_budget,
        seconds(NOW),
        &mut session,
    )
    .await;
    assert!(opened.is_ok());
    session.commit_transaction().await.unwrap();
    assert_eq!(fence(settings().await), before);
    assert_eq!(period_row(&store, &open_platform()).await.unwrap(), row);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn missing_cloud_policy_refuses_with_budget_unavailable_and_metering_closure() {
    let (client, name) = database("platform_no_cloud").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let unconfigured = PlatformBudgetPolicy {
        cloud: None,
        ..open_platform()
    };
    configure_with_platform(&store, &agent.owner, unconfigured.clone(), 1).await;
    assert_eq!(
        store
            .reserve_command(request(&agent, &parent, "unmetered"))
            .await,
        Err(CommandLedgerError::Budget(BudgetError::Unavailable))
    );
    let row = period_row(&store, &unconfigured).await.unwrap();
    assert_eq!(reasons(&row), vec![ClosureReason::MeteringUnavailable]);
    assert_eq!(row.reserved, 0);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_turn_reserved_before_activation_settles_without_touching_the_period_row() {
    let (client, name) = database("platform_activation").await;
    let (store, agent, parent, execution, _, _) = composed(&client, &name).await;
    let policy = configure(&store, &agent.owner).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "pre-activation"))
            .await
            .unwrap(),
    );
    // Reshape the reservation and the row as a predecessor image left them:
    // no period on the record, and a freshly activated row with no liability.
    let command = format!("command:{}", admitted.receipt.envelope.command_id.as_str());
    store
        .model_budget
        .update_one(
            doc! {"_id": &command},
            doc! {"$unset": {"record.period_start": ""}},
        )
        .await
        .unwrap();
    store
        .model_budget
        .delete_one(doc! {"_id": {"$regex": "^platform:"}})
        .await
        .unwrap();
    observe(&store, &open_platform(), 0, seconds(NOW)).await;
    let activated = period_row(&store, &open_platform()).await.unwrap();
    start_turn(&store, execution, &admitted).await;
    finish_turn(&store, &agent).await;
    let owner = store
        .model_budget
        .find_one(doc! {"_id": format!("owner:{}", agent.owner.as_str())})
        .await
        .unwrap()
        .unwrap();
    let owner: layer2_core::budget::OwnerBudget =
        crate::store::sandbox_mongo::read_record(&owner).unwrap();
    assert_eq!((owner.reserved, owner.spent), (0, policy.turn_limit));
    assert_eq!(
        period_row(&store, &open_platform()).await.unwrap(),
        activated
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn lowering_the_cap_below_committed_refuses_the_next_admission_without_releasing() {
    let (client, name) = database("platform_lowered").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let turn_limit = configure_capped(&store, &agent, u64::from(u32::MAX)).await;
    assert!(store
        .reserve_command(request(&agent, &parent, "admitted"))
        .await
        .is_ok());
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"settings.model_budget.platform.period_cap": 10_i64},
            "$inc": {"revision": 1_i64}},
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .reserve_command(request(&agent, &parent, "after-lowering"))
            .await,
        Err(CommandLedgerError::Budget(BudgetError::PlatformExhausted))
    );
    let row = period_row(&store, &capped(10)).await.unwrap();
    assert_eq!(row.reserved, turn_limit);
    let stop = row
        .closures
        .iter()
        .find(|closure| closure.reason == ClosureReason::SpendStop)
        .unwrap();
    assert_eq!(stop.settings_revision, 2);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn settlement_after_period_end_debits_the_original_row() {
    let (client, name) = database("platform_cross_period").await;
    let (store, agent, parent, execution, _, _) = composed(&client, &name).await;
    let policy = configure(&store, &agent.owner).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "spans-periods"))
            .await
            .unwrap(),
    );
    start_turn(&store, execution, &admitted).await;
    // The operator rotates to the next period before the turn settles.
    let next = PlatformBudgetPolicy {
        period_start: "2100-01-01T00:00:00Z".into(),
        period_end: "2101-01-01T00:00:00Z".into(),
        ..open_platform()
    };
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"settings.model_budget.platform":
            mongodb::bson::serialize_to_bson(&next).unwrap()},
              "$inc": {"revision": 1_i64}},
        )
        .await
        .unwrap();
    finish_turn(&store, &agent).await;
    let original = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!((original.reserved, original.spent), (0, policy.turn_limit));
    assert!(period_row(&store, &next).await.is_none());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_lost_runtime_mid_turn_keeps_period_reserved_until_terminal_evidence() {
    let (client, name) = database("platform_lost_runtime").await;
    let (store, agent, parent, execution, _, _) = composed(&client, &name).await;
    let policy = configure(&store, &agent.owner).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "lost-runtime"))
            .await
            .unwrap(),
    );
    start_turn(&store, execution, &admitted).await;
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    let generation = working.conversation.runtime_generation.unwrap();
    // The runtime disappears: no terminal evidence arrives and its usage ages
    // out. The stop path may ask for a cancel; nothing releases liability.
    let later = seconds(&layer2_core::now_rfc3339()) + 3_600;
    use layer2_core::budget::SpendSource;
    let stop = store
        .stop_requested(&agent.id, generation, later)
        .await
        .unwrap();
    assert!(stop.is_some());
    let row = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!((row.reserved, row.spent), (policy.turn_limit, 0));
    finish_turn(&store, &agent).await;
    let row = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!((row.reserved, row.spent), (0, policy.turn_limit));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn disabled_or_absent_platform_policy_changes_nothing() {
    let (client, name) = database("platform_disabled").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    configure(&store, &agent.owner).await;
    store
        .model_budget
        .delete_many(doc! {"_id": {"$regex": "^platform:"}})
        .await
        .unwrap();
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"settings.model_budget.enabled": false}},
        )
        .await
        .unwrap();
    assert!(store
        .reserve_command(request(&agent, &parent, "disabled"))
        .await
        .is_ok());
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$unset": {"settings.model_budget": ""}},
        )
        .await
        .unwrap();
    assert!(store
        .reserve_command(request(&agent, &parent, "absent"))
        .await
        .is_ok());
    assert_eq!(
        store.model_budget.count_documents(doc! {}).await.unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}
