use super::*;
use layer2_core::budget::{Closure, CloudSpendPolicy};

const T0: u64 = 1_788_000_000;

fn stale_after_an_hour() -> PlatformBudgetPolicy {
    PlatformBudgetPolicy {
        cloud: Some(CloudSpendPolicy {
            run_rate_units_per_day: 1,
            max_observation_age_seconds: 3_600,
        }),
        ..open_platform()
    }
}

async fn push(store: &MongoSandboxStore, platform: &PlatformBudgetPolicy, closure: Closure) {
    let (start, _) = platform.period_bounds().unwrap();
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

async fn save_platform(store: &MongoSandboxStore, platform: &PlatformBudgetPolicy) {
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"settings.model_budget.platform":
            mongodb::bson::serialize_to_bson(platform).unwrap()},
              "$inc": {"revision": 1_i64}},
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn evaluation_enters_metering_unavailable_when_the_observation_ages_out_and_lifts_on_a_fresh_one(
) {
    let (client, name) = database("platform_eval_stale").await;
    let (store, agent, _) = fixture(&name, &client).await;
    let platform = stale_after_an_hour();
    configure_with_platform(&store, &agent.owner, platform.clone(), 1).await;
    observe(&store, &platform, 0, T0).await;
    let entered = store.evaluate_period(T0 + 3_601).await.unwrap().unwrap();
    assert_eq!(
        entered
            .entered
            .iter()
            .map(|closure| closure.reason)
            .collect::<Vec<_>>(),
        vec![ClosureReason::MeteringUnavailable]
    );
    let row = period_row(&store, &platform).await.unwrap();
    assert_eq!(reasons(&row), vec![ClosureReason::MeteringUnavailable]);
    observe(&store, &platform, 0, T0 + 3_601).await;
    let lifted = store.evaluate_period(T0 + 3_601).await.unwrap().unwrap();
    assert_eq!(lifted.lifted, vec![ClosureReason::MeteringUnavailable]);
    assert!(period_row(&store, &platform)
        .await
        .unwrap()
        .closures
        .is_empty());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn evaluation_lifts_spend_stop_only_under_a_newer_settings_revision() {
    let (client, name) = database("platform_eval_lift").await;
    let (store, agent, _) = fixture(&name, &client).await;
    configure_metered(&store, &agent.owner).await;
    let platform = open_platform();
    let (start, _) = platform.period_bounds().unwrap();
    push(
        &store,
        &platform,
        Closure {
            reason: ClosureReason::SpendStop,
            since: start,
            settings_revision: 1,
            committed_at_entry: 0,
        },
    )
    .await;
    let now = seconds(NOW);
    let same = store.evaluate_period(now).await.unwrap().unwrap();
    assert!(same.lifted.is_empty());
    assert_eq!(
        reasons(&period_row(&store, &platform).await.unwrap()),
        vec![ClosureReason::SpendStop]
    );
    save_platform(&store, &platform).await;
    let newer = store.evaluate_period(now).await.unwrap().unwrap();
    assert_eq!(newer.lifted, vec![ClosureReason::SpendStop]);
    let row = period_row(&store, &platform).await.unwrap();
    assert!(row.closures.is_empty());
    assert_eq!(row.settings_revision, 2);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn two_evaluators_report_one_newly_entered_closure() {
    let (client, name) = database("platform_eval_two").await;
    let (store, agent, _) = fixture(&name, &client).await;
    let platform = capped(1_000);
    configure_with_platform(&store, &agent.owner, platform.clone(), 1).await;
    // Cloud spend alone crosses the stop level.
    observe(&store, &platform, 1_000, seconds(NOW)).await;
    let peer = MongoSandboxStore::open(&client, &name).await.unwrap();
    let now = seconds(NOW);
    let (left, right) = tokio::join!(store.evaluate_period(now), peer.evaluate_period(now));
    let mut entered: Vec<_> = [left, right]
        .into_iter()
        .flat_map(|evaluation| evaluation.unwrap().unwrap().entered)
        .map(|closure| closure.reason)
        .collect();
    entered.sort_by_key(|reason| format!("{reason:?}"));
    assert_eq!(
        entered,
        vec![ClosureReason::SpendStop, ClosureReason::SpendWarning]
    );
    let row = period_row(&store, &platform).await.unwrap();
    assert_eq!(row.closures.len(), 2);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn the_evaluator_does_not_write_when_nothing_changed() {
    let (client, name) = database("platform_eval_quiet").await;
    let (store, agent, _) = fixture(&name, &client).await;
    configure_metered(&store, &agent.owner).await;
    let now = seconds(NOW);
    store.evaluate_period(now).await.unwrap().unwrap();
    let first = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!(first.evaluated_at, now);
    let quiet = store.evaluate_period(now + 59).await.unwrap().unwrap();
    assert!(!quiet.changed());
    assert_eq!(period_row(&store, &open_platform()).await.unwrap(), first);
    store.evaluate_period(now + 60).await.unwrap().unwrap();
    let refreshed = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!(refreshed.evaluated_at, now + 60);
    assert_eq!(refreshed.revision, first.revision + 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn the_evaluator_pre_creates_the_period_row_outside_admission() {
    let (client, name) = database("platform_eval_create").await;
    let (store, agent, _) = fixture(&name, &client).await;
    configure_with_platform(&store, &agent.owner, open_platform(), 1).await;
    assert!(period_row(&store, &open_platform()).await.is_none());
    store.evaluate_period(seconds(NOW)).await.unwrap().unwrap();
    let row = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!((row.spent, row.reserved), (0, 0));
    // Unobserved cloud spend is unavailable, not zero.
    assert_eq!(reasons(&row), vec![ClosureReason::MeteringUnavailable]);
    assert!(store.evaluate_period(seconds(NOW)).await.unwrap().is_some());
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"settings.model_budget.enabled": false}},
        )
        .await
        .unwrap();
    assert!(store.evaluate_period(seconds(NOW)).await.unwrap().is_none());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn after_a_lift_an_explicit_new_message_is_admitted_and_nothing_replays() {
    let (client, name) = database("platform_eval_resume").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let cap = u64::from(u8::MAX) + 1;
    let turn_limit = configure_capped(&store, &agent, cap).await;
    assert!(store
        .reserve_command(request(&agent, &parent, "admitted"))
        .await
        .is_ok());
    // One observed unit brings committed to disposable, so the refusal
    // below enters the stop that the raised cap then lifts.
    observe(&store, &capped(cap), 1, seconds(NOW)).await;
    let refused = request(&agent, &parent, "refused");
    assert_eq!(
        store.reserve_command(refused.clone()).await,
        Err(CommandLedgerError::Budget(BudgetError::PlatformExhausted))
    );
    assert_eq!(
        store
            .get_admitted_command(&refused.scope(), &refused.key_sha256)
            .await
            .unwrap(),
        None
    );
    save_platform(&store, &capped(4 * turn_limit)).await;
    let lifted = store.evaluate_period(seconds(NOW)).await.unwrap().unwrap();
    assert!(lifted.lifted.contains(&ClosureReason::SpendStop));
    assert!(store
        .reserve_command(request(&agent, &parent, "explicit-new-message"))
        .await
        .is_ok());
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        2,
        "the refused command was never admitted and nothing replays it"
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn cloud_observations_append_once_and_update_the_period_row() {
    use layer2_core::budget::{CloudObservationInput, ObservationError, PlatformSpendLedger};
    let (client, name) = database("platform_observations").await;
    let (store, agent, _) = fixture(&name, &client).await;
    configure_with_platform(&store, &agent.owner, open_platform(), 1).await;
    let now = seconds(NOW);
    let input = CloudObservationInput {
        observed_through: now - 60,
        units: 5,
    };
    let state = store.record_cloud_observation(input, now).await.unwrap();
    let row = state.ledger.unwrap();
    assert_eq!(row.cloud.as_ref().unwrap().units, 5);
    assert_eq!(row.cloud.as_ref().unwrap().settings_revision, 1);
    let repeated = store.record_cloud_observation(input, now).await.unwrap();
    assert_eq!(repeated.ledger.unwrap().revision, row.revision);
    assert_eq!(
        store
            .record_cloud_observation(CloudObservationInput { units: 6, ..input }, now)
            .await,
        Err(ObservationError::Conflict)
    );
    let (start, _) = open_platform().period_bounds().unwrap();
    for invalid in [
        CloudObservationInput {
            observed_through: now + 1,
            units: 1,
        },
        CloudObservationInput {
            observed_through: start - 1,
            units: 1,
        },
        CloudObservationInput {
            observed_through: now - 30,
            units: 1 << 53,
        },
    ] {
        assert_eq!(
            store.record_cloud_observation(invalid, now).await,
            Err(ObservationError::Invalid)
        );
    }
    assert_eq!(
        store
            .model_budget
            .count_documents(doc! {"_id": {"$regex": "^observation:cloud:"}})
            .await
            .unwrap(),
        1
    );
    let later = CloudObservationInput {
        observed_through: now - 30,
        units: 7,
    };
    let state = store.record_cloud_observation(later, now).await.unwrap();
    assert_eq!(state.ledger.unwrap().cloud.unwrap().units, 7);
    assert!(state.committed.is_some());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn adjacent_periods_each_record_an_observation_at_their_shared_boundary() {
    use layer2_core::budget::{CloudObservationInput, PlatformSpendLedger};
    let (client, name) = database("platform_boundary").await;
    let (store, agent, _) = fixture(&name, &client).await;
    let first = PlatformBudgetPolicy {
        period_end: "2026-06-01T00:00:00Z".into(),
        ..open_platform()
    };
    let second = PlatformBudgetPolicy {
        period_start: "2026-06-01T00:00:00Z".into(),
        ..open_platform()
    };
    configure_with_platform(&store, &agent.owner, first.clone(), 1).await;
    let boundary = seconds("2026-06-01T00:00:00Z");
    let now = seconds(NOW);
    store
        .record_cloud_observation(
            CloudObservationInput {
                observed_through: boundary,
                units: 5,
            },
            now,
        )
        .await
        .unwrap();
    save_platform(&store, &second).await;
    let state = store
        .record_cloud_observation(
            CloudObservationInput {
                observed_through: boundary,
                units: 9,
            },
            now,
        )
        .await
        .unwrap();
    assert_eq!(state.ledger.unwrap().cloud.unwrap().units, 9);
    let closed = period_row(&store, &first).await.unwrap();
    assert_eq!(closed.cloud.unwrap().units, 5);
    assert_eq!(
        store
            .model_budget
            .count_documents(doc! {"_id": {"$regex": "^observation:cloud:"}})
            .await
            .unwrap(),
        2
    );
    client.database(&name).drop().await.unwrap();
}
