//! A cached STOP is a poll hint; cancellation needs current durable authority.
use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{
    active_sandbox, database, insert_agent, ready_agent,
};
use layer2_core::budget::{
    BudgetSettings, CloudObservationInput, CloudSpendPolicy, PlatformSpendLedger,
};
use layer2_core::sandbox::AgentStatus;

async fn stopped_fixture() -> (
    mongodb::Client,
    String,
    MongoSandboxStore,
    PlatformBudgetPolicy,
    u64,
) {
    let (client, name, store) = database().await;
    let now = super::super::period::now_seconds();
    let platform = PlatformBudgetPolicy {
        period_cap: 1000,
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
            owners: vec![layer2_core::budget::OwnerBudgetPolicy {
                owner: layer2_core::SubjectId::new("unrelated-budget-owner"),
                policy: layer2_core::budget::BudgetPolicy {
                    owner_day_limit: 1000,
                    turn_limit: 100,
                    max_calls_per_turn: 10,
                    day_timezone: "UTC".into(),
                    input_token_units: 1,
                    output_token_units: 1,
                    max_unobserved_units: 1,
                    usage_timeout_seconds: 60,
                },
            }],
            platform: Some(platform.clone()),
            ..Default::default()
        },
        ..Default::default()
    };
    settings.model_budget.validate().unwrap();
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {
                "$set": {"settings": mongodb::bson::serialize_to_bson(&settings).unwrap()},
                "$inc": {"revision": 1_i64},
            },
        )
        .await
        .unwrap();
    store
        .record_cloud_observation(
            CloudObservationInput {
                observed_through: now,
                units: platform.period_cap,
            },
            now,
        )
        .await
        .unwrap();
    store.evaluate_period(now).await.unwrap();

    (client, name, store, platform, now)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn stale_stop_cache_cannot_create_a_new_turn_receipt_after_lift() {
    let (client, name, store, platform, now) = stopped_fixture().await;
    let sandbox = active_sandbox("sbx_stop_cache");
    let mut prior = ready_agent(&sandbox, "agt_before_lift");
    prior.status = AgentStatus::Working;
    prior.conversation.active_turn = Some(1);
    insert_agent(&store, &prior, &layer2_core::now_rfc3339()).await;
    let stopped = store
        .stop_requested(&prior.id, 1, now)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped.reason, BudgetError::PlatformExhausted);
    assert_eq!(
        store.cached_stop(&period_key(platform.period_bounds().unwrap().0)),
        Some(true)
    );

    // The lift finishes while the process still holds the positive TTL entry.
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {
                "$set": {"settings.model_budget.platform.period_cap": 2000_i64},
                "$inc": {"revision": 1_i64},
            },
        )
        .await
        .unwrap();
    let evaluation = store.evaluate_period(now).await.unwrap().unwrap();
    assert!(evaluation.lifted.contains(&ClosureReason::SpendStop));
    let mut fresh = ready_agent(&sandbox, "agt_after_lift");
    fresh.status = AgentStatus::Working;
    fresh.conversation.active_turn = Some(1);
    insert_agent(&store, &fresh, &layer2_core::now_rfc3339()).await;
    assert_eq!(store.stop_requested(&fresh.id, 1, now).await.unwrap(), None);
    assert!(store
        .model_budget
        .find_one(doc! {
            "_id": format!("stop:{}:1", fresh.id.as_str()),
        })
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store.stop_requested(&prior.id, 1, now).await.unwrap(),
        Some(stopped),
        "an already committed exact-turn receipt survives a lift"
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn platform_stop_authority_conflicts_with_a_lift_or_disable_after_its_snapshot() {
    use crate::store::sandbox_mongo::{is_transient, start_transaction};
    for lift in [true, false] {
        let (client, name, store, platform, now) = stopped_fixture().await;
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        let snapshot =
            super::super::period::read_snapshot(&store.budget_settings, None, &mut session)
                .await
                .unwrap()
                .unwrap();
        assert!(snapshot.platform.is_some());
        let read = super::super::period::read_period(
            &store.model_budget,
            platform.period_bounds().unwrap().0,
            &mut session,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(read
            .ledger
            .closures
            .iter()
            .any(|closure| closure.reason == ClosureReason::SpendStop));
        let change = if lift {
            doc! {"settings.model_budget.platform.period_cap": 2000_i64}
        } else {
            doc! {"settings.model_budget.enabled": false}
        };
        store
            .budget_settings
            .update_one(
                doc! {"_id": "platform"},
                doc! {"$set": change, "$inc": {"revision": 1_i64}},
            )
            .await
            .unwrap();
        if lift {
            store.evaluate_period(now).await.unwrap();
        }
        let result = super::super::stop_authority::fence_stop(&store, &mut session).await;
        assert!(
            matches!(result, Err(super::super::ApplyError::Driver(ref error)) if is_transient(error)),
            "a stale transaction must conflict before it can persist a stop: {result:?}"
        );
        let _ = session.abort_transaction().await;
        client.database(&name).drop().await.unwrap();
    }
}
