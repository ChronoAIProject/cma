//! Deterministic interleavings at the evaluator's persistence boundary.
use super::*;
use layer2_core::budget::{BudgetPolicy, BudgetSettings, ClosureReason, CloudSpendPolicy};

const NOW: u64 = 1_798_848_000;

async fn fixture() -> (
    mongodb::Client,
    String,
    MongoSandboxStore,
    PlatformBudgetPolicy,
) {
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("cma_spend_evaluation_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let platform = PlatformBudgetPolicy {
        period_cap: 10_000,
        period_start: "2026-01-01T00:00:00Z".into(),
        period_end: "2100-01-01T00:00:00Z".into(),
        warning_percent: 80,
        stop_percent: 100,
        protected_reserve: 0,
        lag_allowance: 0,
        cloud: Some(CloudSpendPolicy {
            run_rate_units_per_day: 1,
            max_observation_age_seconds: 3600,
        }),
    };
    let settings = layer2_core::PlatformSettings {
        model_budget: BudgetSettings {
            enabled: true,
            platform: Some(platform.clone()),
            default_policy: Some(BudgetPolicy {
                owner_day_limit: 1000,
                turn_limit: 100,
                max_calls_per_turn: 10,
                day_timezone: "UTC".into(),
                input_token_units: 1,
                output_token_units: 1,
                max_unobserved_units: 1,
                usage_timeout_seconds: 60,
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    store
        .budget_settings
        .replace_one(
            doc! {"_id": "platform"},
            doc! {
                "_id": "platform", "document_schema_version": 1_i32, "revision": 1_i64,
                "settings": mongodb::bson::serialize_to_bson(&settings).unwrap(),
            },
        )
        .upsert(true)
        .await
        .unwrap();
    store
        .record_cloud_observation(
            CloudObservationInput {
                observed_through: NOW,
                units: platform.period_cap,
            },
            NOW,
        )
        .await
        .unwrap();
    (client, name, store, platform)
}

async fn assert_stale_transition_discarded(change: &str) {
    let (client, name, store, platform) = fixture().await;
    let start = platform.period_bounds().unwrap().0;
    let key = period_key(start);
    if change == "release" {
        store
            .model_budget
            .update_one(
                doc! {"_id": &key},
                doc! {"$set": {
                    "record.cloud.units": 0_i64, "record.reserved": platform.period_cap as i64,
                }, "$inc": {"record.revision": 1_i64}},
            )
            .await
            .unwrap();
    }
    let read = store.period_ledger(start).await.unwrap().unwrap();
    let mut decision = read.ledger.clone();
    assert!(decision
        .evaluate(&platform, 1, NOW)
        .unwrap()
        .entered
        .iter()
        .any(|closure| closure.reason == ClosureReason::SpendStop));

    // Another evaluator or accounting writer changes the evidence after this
    // evaluator read it, but before its exact persistence routine is invoked.
    if change == "lift" {
        store.evaluate_period(NOW).await.unwrap();
    }
    if matches!(change, "policy" | "lift") {
        store.budget_settings.update_one(doc! {"_id": "platform"}, doc! {
            "$set": {"settings.model_budget.platform.period_cap": (platform.period_cap * 2) as i64},
            "$inc": {"revision": 1_i64},
        }).await.unwrap();
    } else {
        let field = if change == "release" {
            "record.reserved"
        } else {
            "record.cloud.units"
        };
        store
            .model_budget
            .update_one(
                doc! {"_id": &key},
                doc! {
                    "$set": {field: 0_i64}, "$inc": {"record.revision": 1_i64},
                },
            )
            .await
            .unwrap();
    }
    // Exercise a pure settings race too: the policy changes without a period
    // revision change. Ledger CAS alone cannot fence this case.
    if change != "policy" {
        store.evaluate_period(NOW).await.unwrap();
    }
    let before = store.period_ledger(start).await.unwrap().unwrap();
    let result = store
        .persist_evaluation(&platform, 1, read, NOW)
        .await
        .unwrap()
        .unwrap();
    assert!(
        result.entered.is_empty(),
        "stale {change} decision must be discarded"
    );
    assert_eq!(store.period_ledger(start).await.unwrap().unwrap(), before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn stale_evaluator_cannot_reenter_after_completed_lift() {
    assert_stale_transition_discarded("lift").await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn stale_evaluator_cannot_close_after_observation_correction() {
    assert_stale_transition_discarded("observation").await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn stale_evaluator_cannot_close_after_liability_release() {
    assert_stale_transition_discarded("release").await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn stale_evaluator_cannot_close_after_policy_change_without_ledger_write() {
    assert_stale_transition_discarded("policy").await;
}
