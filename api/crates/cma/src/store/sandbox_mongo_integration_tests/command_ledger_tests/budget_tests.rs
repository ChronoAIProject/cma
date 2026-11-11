use super::*;
use layer2_core::budget::{BudgetError, BudgetPolicy, BudgetSettings, OwnerBudgetPolicy};

fn budget_capabilities() -> layer2_core::RuntimeCapabilities {
    layer2_core::RuntimeCapabilities::new([
        layer2_core::RuntimeCapability::Conversation,
        layer2_core::RuntimeCapability::Usage,
        layer2_core::RuntimeCapability::Interrupt,
    ])
}

pub(super) async fn configure(
    store: &MongoSandboxStore,
    owner: &layer2_core::SubjectId,
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
    let unit = u64::from(u8::MAX);
    let policy = BudgetPolicy {
        owner_day_limit: unit,
        turn_limit: unit,
        max_calls_per_turn: unit,
        day_timezone: "UTC".into(),
        input_token_units: 1,
        output_token_units: 1,
        max_unobserved_units: 1,
        usage_timeout_seconds: 60,
    };
    let settings = layer2_core::PlatformSettings {
        model_budget: BudgetSettings {
            enabled: true,
            platform: Some(layer2_core::budget::PlatformBudgetPolicy {
                period_cap: u64::from(u32::MAX),
                period_start: "2026-11-01T00:00:00Z".into(),
                period_end: "2026-12-01T00:00:00Z".into(),
                warning_percent: 80,
                stop_percent: 100,
                protected_reserve: 0,
                lag_allowance: 0,
                cloud: None,
            }),
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
            doc! {"_id": "platform", "revision": 1_i64,
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "settings": mongodb::bson::serialize_to_bson(&settings).unwrap() },
        )
        .upsert(true)
        .await
        .unwrap();
    policy
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
        Err(CommandLedgerError::Budget(BudgetError::OwnerExhausted))
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
        CommandLedgerError::Budget(BudgetError::OwnerExhausted)
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
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
