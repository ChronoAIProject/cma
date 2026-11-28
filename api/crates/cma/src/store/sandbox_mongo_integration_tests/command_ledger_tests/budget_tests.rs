use super::*;
use layer2_core::budget::{
    BudgetPolicy, BudgetSettings, CloudObservation, CloudSpendPolicy, OwnerBudget,
    OwnerBudgetPolicy, PlatformBudgetPolicy, PlatformPeriodLedger,
};

const OPEN_START: &str = "2026-01-01T00:00:00Z";

pub(super) fn budget_capabilities() -> layer2_core::RuntimeCapabilities {
    layer2_core::RuntimeCapabilities::new([
        layer2_core::RuntimeCapability::Conversation,
        layer2_core::RuntimeCapability::Usage,
        layer2_core::RuntimeCapability::Interrupt,
    ])
}

/// A long period with a configured cloud source whose cap never binds.
pub(super) fn open_platform() -> PlatformBudgetPolicy {
    PlatformBudgetPolicy {
        period_cap: u64::from(u32::MAX),
        period_start: OPEN_START.into(),
        period_end: "2100-01-01T00:00:00Z".into(),
        warning_percent: 80,
        stop_percent: 100,
        protected_reserve: 0,
        lag_allowance: 0,
        cloud: Some(CloudSpendPolicy {
            run_rate_units_per_day: 1,
            max_observation_age_seconds: (1 << 53) - 1,
        }),
    }
}

fn budget_policy() -> BudgetPolicy {
    let unit = u64::from(u8::MAX);
    BudgetPolicy {
        owner_day_limit: unit,
        turn_limit: unit,
        max_calls_per_turn: unit,
        day_timezone: "UTC".into(),
        input_token_units: 1,
        output_token_units: 1,
        max_unobserved_units: 1,
        usage_timeout_seconds: 60,
    }
}

/// Owner budgets exactly as #1621 configures them: a platform policy with no
/// cloud source, so platform spend metering is not enabled (decision 31).
pub(super) async fn configure(
    store: &MongoSandboxStore,
    owner: &layer2_core::SubjectId,
) -> BudgetPolicy {
    let platform = PlatformBudgetPolicy {
        period_cap: u64::from(u32::MAX),
        period_start: "2026-11-01T00:00:00Z".into(),
        period_end: "2026-12-01T00:00:00Z".into(),
        warning_percent: 80,
        stop_percent: 100,
        protected_reserve: 0,
        lag_allowance: 0,
        cloud: None,
    };
    configure_with_platform(store, owner, platform, 1).await
}

/// Owner budgets under an open, observed, metered platform period.
pub(super) async fn configure_metered(
    store: &MongoSandboxStore,
    owner: &layer2_core::SubjectId,
) -> BudgetPolicy {
    let policy = configure(store, owner).await;
    meter(store).await;
    policy
}

/// Enable platform metering on an existing configuration: the open period with
/// a cloud source and a zero observation at its start.
pub(super) async fn meter(store: &MongoSandboxStore) {
    let platform = open_platform();
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"settings.model_budget.platform":
            mongodb::bson::serialize_to_bson(&platform).unwrap()}},
        )
        .await
        .unwrap();
    let start = layer2_core::seconds_from_rfc3339(OPEN_START).unwrap();
    observe(store, &platform, 0, start).await;
}

pub(super) async fn configure_with_platform(
    store: &MongoSandboxStore,
    owner: &layer2_core::SubjectId,
    platform: PlatformBudgetPolicy,
    revision: u64,
) -> BudgetPolicy {
    let mut cursor = store
        .agents
        .find(doc! {"owner": owner.as_str()})
        .await
        .unwrap();
    while let Some(document) = cursor.try_next().await.unwrap() {
        let mut agent = crate::store::agents::read_agent(&document).unwrap();
        agent.capabilities = budget_capabilities();
        store
            .agents
            .replace_one(
                doc! {"_id": agent.id.as_str()},
                crate::store::agents::agent_document(&agent).unwrap(),
            )
            .await
            .unwrap();
    }
    let policy = budget_policy();
    let settings = layer2_core::PlatformSettings {
        model_budget: BudgetSettings {
            enabled: true,
            platform: Some(platform),
            default_policy: None,
            request_models: Vec::new(),
            owners: vec![OwnerBudgetPolicy {
                owner: owner.clone(),
                policy: policy.clone(),
            }],
        },
        ..Default::default()
    };
    store
        .budget_settings
        .replace_one(
            doc! {"_id": "platform"},
            doc! {"_id": "platform", "revision": revision as i64,
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "settings": mongodb::bson::serialize_to_bson(&settings).unwrap() },
        )
        .upsert(true)
        .await
        .unwrap();
    policy
}

/// Fixture stand-in for the operator observation route: record the latest
/// cloud observation directly on the period row.
pub(super) async fn observe(
    store: &MongoSandboxStore,
    platform: &PlatformBudgetPolicy,
    units: u64,
    observed_through: u64,
) {
    let ledger = PlatformPeriodLedger::open(platform, 0).unwrap();
    let key = crate::store::budget::period_key(ledger.period_start);
    store
        .model_budget
        .update_one(
            doc! {"_id": &key},
            doc! {"$setOnInsert": {"document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "record": mongodb::bson::serialize_to_bson(&ledger).unwrap()}},
        )
        .upsert(true)
        .await
        .unwrap();
    let observation = CloudObservation {
        observed_through,
        units,
        recorded_at: observed_through,
        settings_revision: 0,
    };
    store
        .model_budget
        .update_one(
            doc! {"_id": &key},
            doc! {"$set": {"record.cloud": mongodb::bson::serialize_to_bson(&observation).unwrap()},
            "$inc": {"record.revision": 1_i64}},
        )
        .await
        .unwrap();
}

pub(super) async fn period_row(
    store: &MongoSandboxStore,
    platform: &PlatformBudgetPolicy,
) -> Option<PlatformPeriodLedger> {
    let (start, _) = platform.period_bounds().unwrap();
    store
        .model_budget
        .find_one(doc! {"_id": crate::store::budget::period_key(start)})
        .await
        .unwrap()
        .map(|document| crate::store::sandbox_mongo::read_record(&document).unwrap())
}

pub(super) async fn add_owner(store: &MongoSandboxStore, owner: &layer2_core::SubjectId) {
    let entry = OwnerBudgetPolicy {
        owner: owner.clone(),
        policy: budget_policy(),
    };
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$push": {"settings.model_budget.owners":
            mongodb::bson::serialize_to_bson(&entry).unwrap()}},
        )
        .await
        .unwrap();
}

pub(super) async fn raise_owner_limit(store: &MongoSandboxStore, factor: u64) {
    let limit = budget_policy().owner_day_limit * factor;
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"settings.model_budget.owners.0.policy.owner_day_limit": limit as i64}},
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn model_budget_reserves_once_and_replay_survives_exhaustion_and_restart() {
    let (client, name) = database("budget_replay").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    configure(&store, &agent.owner).await;
    let first = request(&agent, &parent, "budget-original");
    let admitted = record(store.reserve_command(first.clone()).await.unwrap());
    assert_eq!(
        store
            .reserve_command(request(&agent, &parent, "budget-refused"))
            .await,
        Err(CommandLedgerError::OwnerBudgetExhausted {
            reset_at: layer2_core::seconds_from_rfc3339("2026-09-06T00:00:00Z").unwrap(),
        })
    );
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let replay = record(reopened.reserve_command(first).await.unwrap());
    assert_eq!(replay, admitted);
    assert_eq!(
        reopened
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        reopened
            .model_budget
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_model_budget_reservations_serialize_across_store_instances() {
    let (client, name) = database("budget_concurrent").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    configure(&store, &agent.owner).await;
    let mut second_agent = ready_agent(&parent, "agt_budget_competitor");
    second_agent.capabilities = budget_capabilities();
    insert_agent(&store, &second_agent, NOW).await;
    let peer = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (first, second) = tokio::join!(
        store.reserve_command(request(&agent, &parent, "budget-first")),
        peer.reserve_command(request(&second_agent, &parent, "budget-second")),
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let refused = first.err().or(second.err()).unwrap();
    assert_eq!(
        refused,
        CommandLedgerError::OwnerBudgetExhausted {
            reset_at: layer2_core::seconds_from_rfc3339("2026-09-06T00:00:00Z").unwrap(),
        }
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owner_budget_refusal_announces_the_utc_rollover_that_allows_new_work() {
    let (client, name) = database("budget_utc_rollover").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let policy = configure(&store, &agent.owner).await;
    let reset = "2026-09-06T00:00:00Z";
    let before = "2026-09-05T23:59:59Z";
    let reset_at = layer2_core::seconds_from_rfc3339(reset).unwrap();
    let owner = OwnerBudget {
        day: policy.day(reset_at - 1).unwrap(),
        spent: policy.owner_day_limit,
        reserved: 0,
        token_units: Some([policy.input_token_units, policy.output_token_units]),
        day_timezone: Some(policy.day_timezone.clone()),
    };
    let owner_key = format!("owner:{}", agent.owner);
    store
        .model_budget
        .insert_one(doc! {
            "_id": &owner_key, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "record": mongodb::bson::serialize_to_document(&owner).unwrap(),
        })
        .await
        .unwrap();
    let mut input = request(&agent, &parent, "budget-window-boundary");
    input.admitted_at = before.into();
    input.queue_deadline = "2026-09-06T00:02:00Z".into();
    assert_eq!(
        store.reserve_command(input.clone()).await,
        Err(CommandLedgerError::OwnerBudgetExhausted { reset_at }),
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    assert_eq!(
        store.model_budget.count_documents(doc! {}).await.unwrap(),
        1
    );

    input.admitted_at = reset.into();
    let admitted = record(store.reserve_command(input.clone()).await.unwrap());
    assert_eq!(
        record(store.reserve_command(input).await.unwrap()),
        admitted
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );
    let stored = store
        .model_budget
        .find_one(doc! { "_id": &owner_key })
        .await
        .unwrap()
        .unwrap();
    let charged: OwnerBudget = crate::store::sandbox_mongo::read_record(&stored).unwrap();
    assert_eq!(
        (charged.day, charged.spent, charged.reserved),
        (owner.day + 1, 0, policy.turn_limit)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn aborted_model_budget_transaction_releases_its_reservation() {
    let (client, name) = database("budget_rollback").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    configure(&store, &agent.owner).await;
    let current = store.get_agent(&agent.id).await.unwrap().unwrap();
    let mut session = client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    crate::store::budget::reserve_command(
        &store,
        &current,
        "rolled-back-command",
        layer2_core::seconds_from_rfc3339(NOW).unwrap(),
        layer2_core::admission::AdmissionCause::Fresh,
        false,
        &mut session,
    )
    .await
    .unwrap_or_else(|_| panic!("reservation must succeed in transaction"));
    session.abort_transaction().await.unwrap();
    assert_eq!(
        store.model_budget.count_documents(doc! {}).await.unwrap(),
        0
    );
    assert!(store
        .reserve_command(request(&agent, &parent, "budget-after-abort"))
        .await
        .is_ok());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn model_budget_scope_never_consumes_another_owners_allowance() {
    let (client, name) = database("budget_scope").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    configure(&store, &agent.owner).await;
    let mut other_parent = active_sandbox("sbx_budget_other");
    other_parent.owner = layer2_core::SubjectId("budget-other-owner".into());
    store.create_sandbox(other_parent.clone()).await.unwrap();
    let other_agent = ready_agent(&other_parent, "agt_budget_other");
    insert_agent(&store, &other_agent, NOW).await;
    assert!(store
        .reserve_command(request(&agent, &parent, "budget-full"))
        .await
        .is_ok());
    assert!(store
        .reserve_command(request(&other_agent, &other_parent, "budget-other"))
        .await
        .is_ok());
    assert_eq!(
        store.model_budget.count_documents(doc! {}).await.unwrap(),
        2
    );
    client.database(&name).drop().await.unwrap();
}
