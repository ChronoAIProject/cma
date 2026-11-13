//! The platform period row moves inside the owner reservation transactions.
use super::super::budget_tests::{
    add_owner, budget_capabilities, configure_metered, configure_with_platform, meter, observe,
    open_platform, period_row, raise_owner_limit,
};
use super::*;
use layer2_core::budget::{BudgetError, ClosureReason, PlatformBudgetPolicy, SpendSource};
use layer3_adapter::{ConversationUpdate, PromptResponse, TurnStopReason};

#[path = "platform_counter_tests.rs"]
mod counters;

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

fn wall_clock() -> u64 {
    seconds(&layer2_core::now_rfc3339())
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
    let policy = configure_metered(&store, &agent.owner).await;
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
async fn an_aborted_reservation_transaction_leaves_the_period_row_untouched() {
    let (client, name) = database("platform_rollback").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    configure_metered(&store, &agent.owner).await;
    let before = period_row(&store, &open_platform()).await.unwrap();
    let current = store.get_agent(&agent.id).await.unwrap().unwrap();
    let mut session = client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    crate::store::budget::reserve_command(
        &store,
        &current,
        "rolled-back-command",
        seconds(NOW),
        layer2_core::admission::AdmissionCause::Fresh,
        false,
        &mut session,
    )
    .await
    .unwrap_or_else(|_| panic!("reservation must succeed in transaction"));
    session.abort_transaction().await.unwrap();
    assert_eq!(period_row(&store, &open_platform()).await.unwrap(), before);
    assert!(store
        .reserve_command(request(&agent, &parent, "after-abort"))
        .await
        .is_ok());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn request_hold_mirrors_and_settles_against_the_period_row() {
    use layer2_core::budget::{RequestBudget, RequestUsage};
    let (client, name) = database("platform_request_mirror").await;
    let (store, scope, request) = super::request_budget::completed(&client, &name).await;
    meter(&store).await;
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
    let policy = configure_metered(&store, &agent.owner).await;
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
async fn an_allowance_that_does_not_fit_is_refused_and_running_work_stops_only_at_disposable() {
    let (client, name) = database("platform_refusal").await;
    let (store, agent, parent, execution, _, _) = composed(&client, &name).await;
    // Disposable is one allowance plus two units; the zero observation at the
    // wall clock projects at most one unit while the turn runs.
    let platform = capped(u64::from(u8::MAX) + 2);
    let turn_limit = configure_with_platform(&store, &agent.owner, platform.clone(), 1)
        .await
        .turn_limit;
    observe(&store, &platform, 0, wall_clock()).await;
    raise_owner_limit(&store, 64).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "fits"))
            .await
            .unwrap(),
    );
    start_turn(&store, execution, &admitted).await;
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    let generation = working.conversation.runtime_generation.unwrap();
    assert_eq!(
        store
            .reserve_command(live_request(&working, &parent, "overshoots"))
            .await,
        Err(CommandLedgerError::Budget(BudgetError::PlatformExhausted))
    );
    let row = period_row(&store, &platform).await.unwrap();
    assert_eq!(row.reserved, turn_limit);
    assert_eq!(reasons(&row), vec![ClosureReason::SpendWarning]);
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );
    assert_eq!(
        store
            .stop_requested(&agent.id, generation, wall_clock())
            .await
            .unwrap(),
        None,
        "the refused allowance does not stop the running turn"
    );
    // An observation brings committed to disposable; only then does the
    // evaluator enter the stop and the running turn get its cancel.
    let now = wall_clock();
    observe(&store, &platform, 2, now).await;
    let evaluation = store.evaluate_period(now).await.unwrap().unwrap();
    assert_eq!(
        evaluation
            .entered
            .iter()
            .map(|closure| closure.reason)
            .collect::<Vec<_>>(),
        vec![ClosureReason::SpendStop]
    );
    store.age_platform_stop_cache(std::time::Duration::from_secs(5));
    assert_eq!(
        store
            .stop_requested(&agent.id, generation, now)
            .await
            .unwrap()
            .map(|stop| stop.reason),
        Some(BudgetError::PlatformExhausted)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_refused_admission_leaves_one_spend_stop_visible_after_its_transaction_aborted() {
    let (client, name) = database("platform_refusal_visible").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    // Observed cloud spend alone already reaches disposable: the very first
    // liability is refused on a row that carries no liability.
    let cap = u64::from(u8::MAX) - 1;
    configure_capped(&store, &agent, cap).await;
    observe(&store, &capped(cap), cap, seconds(NOW)).await;
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
    let cap = u64::from(u8::MAX) + 1;
    let turn_limit = configure_capped(&store, &agent, cap).await;
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
    let row = period_row(&store, &capped(cap)).await.unwrap();
    assert_eq!(row.reserved, turn_limit);
    assert_eq!(
        reasons(&row),
        vec![ClosureReason::SpendWarning],
        "the allowance that did not fit stopped nothing"
    );
    // One more observed unit brings committed to disposable: both stores'
    // next admissions are refused and exactly one stop is recorded. Both come
    // from the owner with a raised day limit, so whichever owner won above,
    // the owner check cannot refuse first.
    observe(&store, &capped(cap), 1, seconds(NOW)).await;
    let (first, second) = tokio::join!(
        store.reserve_command(request(&agent, &parent, "third")),
        peer.reserve_command(request(&agent, &parent, "fourth")),
    );
    for refused in [first, second] {
        assert_eq!(
            refused,
            Err(CommandLedgerError::Budget(BudgetError::PlatformExhausted))
        );
    }
    let row = period_row(&store, &capped(cap)).await.unwrap();
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
    configure_metered(&store, &agent.owner).await;
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
    configure_metered(&store, &agent.owner).await;
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
        layer2_core::admission::AdmissionCause::Fresh,
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
async fn without_a_cloud_source_platform_metering_is_off_and_refuses_nothing() {
    use layer2_core::budget::{PlatformAdmissionState, PlatformSpendLedger};
    let (client, name) = database("platform_no_cloud").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let unmetered = PlatformBudgetPolicy {
        cloud: None,
        ..open_platform()
    };
    configure_with_platform(&store, &agent.owner, unmetered.clone(), 1).await;
    assert!(store
        .reserve_command(request(&agent, &parent, "unmetered"))
        .await
        .is_ok());
    let now = seconds(NOW);
    assert_eq!(store.evaluate_period(now).await.unwrap(), None);
    assert_eq!(
        store.admission_state(now).await.unwrap(),
        PlatformAdmissionState::default()
    );
    let spend = store.spend_state(now).await.unwrap();
    assert!(!spend.metering_enabled);
    assert!(spend.ledger.is_none() && spend.levels.is_none());
    assert!(period_row(&store, &unmetered).await.is_none());
    assert_eq!(
        store
            .model_budget
            .count_documents(doc! {"_id": {"$regex": "^platform:"}})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[path = "platform_period_tests/evaluation.rs"]
mod evaluation;

#[path = "platform_period_tests/settlement.rs"]
mod settlement;
