use super::*;
use layer2_core::budget::{BudgetError, ModelRequest, RequestBudget, RequestScope, RequestUsage};
use layer3_adapter::{ConversationUpdate, PromptResponse, TurnStopReason};

#[path = "request_budget_tests/http_tests.rs"]
mod http_tests;
#[path = "request_budget_tests/ordinary_parent.rs"]
mod ordinary_parent;
#[path = "request_budget_tests/settlement_tests.rs"]
mod settlement_tests;

pub(in crate::store) async fn completed(
    client: &Client,
    name: &str,
) -> (Arc<MongoSandboxStore>, RequestScope, ModelRequest) {
    let (store, scope, request) = completed_without_finality(client, name, None).await;
    // Fixture for the future authoritative gateway settlement contract. CMA's
    // ordinary runtime terminal events never assert these finality fields.
    store
        .model_budget
        .update_one(
            doc! {"_id": format!("turn:{}:{}", scope.agent_id, scope.turn_sequence)},
            doc! {"$set": {"record.reservation.final_usage_known": true,
            "record.reservation.final_model_calls": 1_i64}},
        )
        .await
        .unwrap();
    (store, scope, request)
}

async fn completed_without_finality(
    client: &Client,
    name: &str,
    turn_limit: Option<u64>,
) -> (Arc<MongoSandboxStore>, RequestScope, ModelRequest) {
    let (store, agent, parent, execution, _, _) = composed(client, name).await;
    let policy = super::super::budget_tests::configure(&store, &agent.owner).await;
    let mut limits = doc! {
        "settings.model_budget.owners.0.policy.owner_day_limit": (turn_limit.unwrap_or(policy.owner_day_limit) * 4) as i64,
        "settings.model_budget.request_models": [{"model": "request-fixture", "input_overhead_tokens": 1_i64}]
    };
    if let Some(turn_limit) = turn_limit {
        limits.insert(
            "settings.model_budget.owners.0.policy.turn_limit",
            turn_limit as i64,
        );
    }
    store
        .budget_settings
        .update_one(doc! {"_id": "platform"}, doc! {"$set": limits})
        .await
        .unwrap();
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "parent-turn"))
            .await
            .unwrap(),
    );
    AgentCommandWorker::new(store.clone(), execution, "request-worker".into())
        .unwrap()
        .run_one(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    let turn = working.conversation.active_turn.unwrap();
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
    let scope = RequestScope {
        agent_id: agent.id,
        turn_sequence: turn,
        request_id: "auxiliary-first".into(),
    };
    let request = ModelRequest {
        model: "request-fixture".into(),
        input_bytes: turn_limit.unwrap_or(policy.turn_limit) / 2,
        max_output_tokens: 1,
        fingerprint: "b".repeat(64),
    };
    (store, scope, request)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn uncertain_parent_usage_or_call_count_cannot_admit_auxiliary_spend() {
    let (client, name) = database("request_parent_uncertain").await;
    let (store, scope, request) = completed(&client, &name).await;
    let key = format!("turn:{}:{}", scope.agent_id.as_str(), scope.turn_sequence);
    store
        .model_budget
        .update_one(
            doc! {"_id": &key},
            doc! {"$set": {"record.reservation.final_usage_known": false}},
        )
        .await
        .unwrap();
    assert_eq!(
        store.reserve_request(&scope, &request).await,
        Err(BudgetError::TurnExhausted)
    );
    store
        .model_budget
        .update_one(
            doc! {"_id": &key},
            doc! {
                "$set": {"record.reservation.final_usage_known": true},
                "$unset": {"record.reservation.final_model_calls": ""}
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store.reserve_request(&scope, &request).await,
        Err(BudgetError::Unavailable)
    );
    assert_eq!(
        store
            .model_budget
            .count_documents(doc! {"_id": {"$regex": "^request:"}})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn request_call_limit_includes_parent_and_survives_settlement_and_restart() {
    let (client, name) = database("request_call_limit").await;
    let (store, scope, mut request) = completed(&client, &name).await;
    request.input_bytes = 1;
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {
                "settings.model_budget.owners.0.policy.max_calls_per_turn": 3_i64
            }},
        )
        .await
        .unwrap();
    let mut second = scope.clone();
    second.request_id = "second-counted-call".into();
    let peer = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (first, second) = tokio::join!(
        store.reserve_request(&scope, &request),
        peer.reserve_request(&second, &request)
    );
    for hold in [first.unwrap().unwrap(), second.unwrap().unwrap()] {
        store
            .settle_request(
                &hold,
                RequestUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                },
            )
            .await
            .unwrap();
    }
    let mut next = scope;
    next.request_id = "excess-counted-call".into();
    let restarted = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        restarted.reserve_request(&next, &request).await,
        Err(BudgetError::TurnExhausted)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn unknown_request_commit_never_returns_another_send_permit() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cma_request_commit_{suffix}");
    let app = format!("cma-request-commit-{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let (store, scope, request) = completed(&client, &name).await;
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":5_i32},
        "data":{"failCommands":["commitTransaction"], "appName":&app,
            "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected request commit ambiguity"}}
    }).await.unwrap();
    let result = store.reserve_request(&scope, &request).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert_eq!(result, Err(BudgetError::Unavailable));
    crate::store::test_visibility::majority_barrier(&admin, &name).await;
    let reopened = MongoSandboxStore::open(&admin, &name).await.unwrap();
    assert_eq!(
        reopened.reserve_request(&scope, &request).await,
        Err(BudgetError::Unavailable)
    );
    assert_eq!(
        reopened
            .model_budget
            .count_documents(doc! {"_id":{"$regex":"^request:"}})
            .await
            .unwrap(),
        1
    );
    admin.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn request_holds_serialize_before_send_and_survive_restart_without_usage() {
    let (client, name) = database("request_concurrent").await;
    let (store, first, request) = completed(&client, &name).await;
    let mut second = first.clone();
    second.request_id = "auxiliary-second".into();
    let peer = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (a, b) = tokio::join!(
        store.reserve_request(&first, &request),
        peer.reserve_request(&second, &request)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(
        a.as_ref().err().or(b.as_ref().err()),
        Some(&BudgetError::TurnExhausted)
    );
    let hold = a.ok().or(b.ok()).unwrap().unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.reserve_request(&hold.scope, &request).await,
        Err(BudgetError::Unavailable)
    );
    let mut next = first;
    next.request_id = "after-restart".into();
    assert_eq!(
        reopened.reserve_request(&next, &request).await,
        Err(BudgetError::TurnExhausted)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn recorded_request_usage_settles_once_and_releases_only_unused_liability() {
    let (client, name) = database("request_settlement").await;
    let (store, scope, request) = completed(&client, &name).await;
    let hold = store
        .reserve_request(&scope, &request)
        .await
        .unwrap()
        .unwrap();
    let usage = RequestUsage {
        input_tokens: 7,
        output_tokens: 1,
    };
    store.settle_request(&hold, usage).await.unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    reopened.settle_request(&hold, usage).await.unwrap();
    assert_eq!(
        reopened
            .settle_request(
                &hold,
                RequestUsage {
                    input_tokens: 8,
                    output_tokens: 1
                }
            )
            .await,
        Err(BudgetError::Unavailable)
    );
    let mut next = scope;
    next.request_id = "next-attempt".into();
    assert!(reopened
        .reserve_request(&next, &request)
        .await
        .unwrap()
        .is_some());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unpriced_unbound_or_unavailable_requests_never_create_send_permits() {
    let (client, name) = database("request_refusals").await;
    let (store, scope, request) = completed(&client, &name).await;
    let mut unknown = request.clone();
    unknown.model = "unapproved-model".into();
    assert_eq!(
        store.reserve_request(&scope, &unknown).await,
        Err(BudgetError::Unavailable)
    );
    let mut unbound = scope.clone();
    unbound.turn_sequence += 1;
    assert_eq!(
        store.reserve_request(&unbound, &request).await,
        Err(BudgetError::Unavailable)
    );
    store
        .budget_settings
        .delete_one(doc! {"_id": "platform"})
        .await
        .unwrap();
    assert_eq!(
        store.reserve_request(&scope, &request).await,
        Err(BudgetError::Unavailable)
    );
    assert_eq!(
        store
            .model_budget
            .count_documents(doc! {"_id": {"$regex": "^request:"}})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provider_bound_violation_is_accounted_and_blocks_further_auxiliary_calls() {
    let (client, name) = database("request_violation").await;
    let (store, scope, request) = completed(&client, &name).await;
    let hold = store
        .reserve_request(&scope, &request)
        .await
        .unwrap()
        .unwrap();
    store
        .settle_request(
            &hold,
            RequestUsage {
                input_tokens: 1,
                output_tokens: request.max_output_tokens + 1,
            },
        )
        .await
        .unwrap();
    let mut next = scope;
    next.request_id = "after-violation".into();
    assert_eq!(
        store.reserve_request(&next, &request).await,
        Err(BudgetError::Unavailable)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn request_owner_exhaustion_refuses_and_confirmed_disabled_scope_preserves_rollout() {
    let (client, name) = database("request_owner_limit").await;
    let (store, scope, request) = completed(&client, &name).await;
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {
                "settings.model_budget.owners.0.policy.owner_day_limit": i64::from(u8::MAX)
            }},
        )
        .await
        .unwrap();
    assert_eq!(
        store.reserve_request(&scope, &request).await,
        Err(BudgetError::OwnerExhausted)
    );
    assert_eq!(
        store
            .model_budget
            .count_documents(doc! {"_id": {"$regex": "^request:"}})
            .await
            .unwrap(),
        0
    );
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$unset": {
                "settings.model_budget": ""
            }},
        )
        .await
        .unwrap();
    assert_eq!(store.reserve_request(&scope, &request).await.unwrap(), None);
    assert_eq!(
        store
            .model_budget
            .count_documents(doc! {"_id": {"$regex": "^request:"}})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}
