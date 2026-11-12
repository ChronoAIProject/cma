//! Persistence guarantees for the opt-in bounded counter layout.
use super::counter_accounting::Charge;
use super::counters::Change;
use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, CommitOutcome, TRANSACTION_ATTEMPTS,
};
use layer2_core::budget::{
    BudgetSettings, CloudObservationInput, CloudSpendPolicy, PlatformBudgetPolicy,
    PlatformSpendLedger,
};

const NOW: u64 = 1_798_848_000;

#[path = "counter_race_tests.rs"]
mod races;

async fn fixture(
    cap: u64,
    enabled: bool,
) -> (
    mongodb::Client,
    String,
    MongoSandboxStore,
    PlatformBudgetPolicy,
) {
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    fixture_with_client(client, cap, enabled).await
}

async fn fixture_with_client(
    client: mongodb::Client,
    cap: u64,
    enabled: bool,
) -> (
    mongodb::Client,
    String,
    MongoSandboxStore,
    PlatformBudgetPolicy,
) {
    let name = format!("cma_platform_counter_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .with_platform_counter_shards(enabled);
    let policy = PlatformBudgetPolicy {
        period_cap: cap,
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
            platform: Some(policy.clone()),
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
                observed_through: NOW,
                units: 0,
            },
            NOW,
        )
        .await
        .unwrap();
    (client, name, store, policy)
}

async fn finish<T>(
    session: &mut ClientSession,
    result: Result<T, ApplyError>,
) -> Option<Result<T, BudgetError>> {
    match result {
        Ok(value) => match commit_transaction(session).await {
            CommitOutcome::Committed => Some(Ok(value)),
            CommitOutcome::Retry(_) => None,
            CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                panic!("unexpected commit failure: {error}")
            }
        },
        Err(error) => {
            let _ = session.abort_transaction().await;
            match error {
                ApplyError::Driver(error) if is_transient(&error) => None,
                ApplyError::Domain(layer2_core::sandbox::store::SandboxStoreError::Budget(
                    reason,
                )) => Some(Err(reason)),
                other => panic!("unexpected accounting failure: {other:?}"),
            }
        }
    }
}

async fn reserve(
    store: &MongoSandboxStore,
    owner: &str,
    amount: u64,
) -> Result<Charge, BudgetError> {
    let owner = layer2_core::SubjectId::new(owner);
    let _guard = store.platform_write_guard().await;
    for attempt in 0..TRANSACTION_ATTEMPTS {
        crate::store::contention::backoff(attempt).await;
        let mut session = store.client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        let result = async {
            let snapshot = period::fenced_snapshot(store, &owner, &mut session).await?;
            period::admit_liability(store, &snapshot, &owner, amount, NOW, &mut session).await
        }
        .await;
        if let Some(result) = finish(&mut session, result).await {
            return result.map(Option::unwrap);
        }
    }
    panic!("counter reservation exhausted transaction retries")
}

async fn change(store: &MongoSandboxStore, charge: Charge, change: Change) {
    let _guard = store.platform_settlement_guard().await;
    for attempt in 0..TRANSACTION_ATTEMPTS {
        crate::store::contention::backoff(attempt).await;
        let mut session = store.client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        let result = counter_accounting::mirror(
            store,
            Some(charge.start),
            charge.shard,
            change,
            &mut session,
        )
        .await;
        if let Some(result) = finish(&mut session, result).await {
            result.unwrap();
            return;
        }
    }
    panic!("counter settlement exhausted transaction retries")
}

async fn root(store: &MongoSandboxStore, policy: &PlatformBudgetPolicy) -> Document {
    store
        .model_budget
        .find_one(doc! {"_id": period::period_key(policy.period_bounds().unwrap().0)})
        .await
        .unwrap()
        .unwrap()
}

fn owners() -> (String, String) {
    let first = "counter-owner-0".to_owned();
    let shard = counters::partition(&layer2_core::SubjectId::new(&first));
    let second = (1..100)
        .map(|n| format!("counter-owner-{n}"))
        .find(|owner| counters::partition(&layer2_core::SubjectId::new(owner)) != shard)
        .unwrap();
    (first, second)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn distributed_counters_remove_routine_period_writes_and_retain_exact_totals() {
    let (client, name, store, policy) = fixture(10_000, true).await;
    let (first, second) = owners();
    let a = reserve(&store, &first, 100).await.unwrap();
    let before = root(&store, &policy).await;
    let b = reserve(&store, &second, 100).await.unwrap();
    change(
        &store,
        a,
        Change {
            released: 100,
            debit: 40,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        root(&store, &policy).await,
        before,
        "routine reservation and settlement must not write the shared period"
    );
    assert_ne!(a.shard, b.shard);
    let ledger = store.spend_state(NOW).await.unwrap().ledger.unwrap();
    assert_eq!((ledger.spent, ledger.reserved), (40, 100));
    assert!(
        decode::<layer2_core::budget::PlatformPeriodLedger>(&before).is_err(),
        "an old strict reader must refuse distributed balances"
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn distributed_counters_serialize_competing_admissions_across_store_instances() {
    let (client, name, store, _) = fixture(150, true).await;
    // Complete conversion before introducing the deliberate admission race.
    let seed = reserve(&store, "seed", 1).await.unwrap();
    change(
        &store,
        seed,
        Change {
            released: 1,
            ..Default::default()
        },
    )
    .await;
    let other = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .with_platform_counter_shards(true);
    let (a, b) = owners();
    let (left, right) = tokio::join!(reserve(&store, &a, 100), reserve(&other, &b, 100));
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert!(
        matches!(left, Err(BudgetError::PlatformExhausted))
            || matches!(right, Err(BudgetError::PlatformExhausted))
    );
    let ledger = store.spend_state(NOW).await.unwrap().ledger.unwrap();
    assert_eq!(ledger.reserved, 100);
    assert!(
        !ledger.closed_for_commitments(),
        "an allowance that does not fit cannot create STOP"
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn distributed_counters_preserve_legacy_obligations_and_gate_off_recovery() {
    let (client, name, legacy, policy) = fixture(10_000, false).await;
    let old = reserve(&legacy, "old", 100).await.unwrap();
    assert!(old.shard.is_none());
    assert!(!root(&legacy, &policy)
        .await
        .get_document("record")
        .unwrap()
        .contains_key("counter_layout"));
    let enabled = legacy.clone().with_platform_counter_shards(true);
    let new = reserve(&enabled, "new", 100).await.unwrap();
    assert!(new.shard.is_some());
    let restart = MongoSandboxStore::open(&client, &name).await.unwrap();
    change(
        &restart,
        old,
        Change {
            released: 100,
            debit: 60,
            ..Default::default()
        },
    )
    .await;
    change(
        &restart,
        new,
        Change {
            released: 100,
            ..Default::default()
        },
    )
    .await;
    let ledger = restart.spend_state(NOW).await.unwrap().ledger.unwrap();
    assert_eq!((ledger.spent, ledger.reserved), (60, 0));
    assert!(reserve(&restart, "later", 100)
        .await
        .unwrap()
        .shard
        .is_some());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn distributed_counters_discard_a_stop_decision_after_a_shard_release() {
    let (client, name, store, policy) = fixture(100, true).await;
    let held = reserve(&store, "held", 100).await.unwrap();
    let read = store.period_ledger(held.start).await.unwrap().unwrap();
    let before_root = root(&store, &policy).await;
    change(
        &store,
        held,
        Change {
            released: 100,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(root(&store, &policy).await, before_root);
    let evaluation = store
        .persist_evaluation(&policy, 1, read, NOW)
        .await
        .unwrap()
        .unwrap();
    assert!(
        evaluation.entered.is_empty(),
        "a changed shard invalidates the captured closure decision"
    );
    assert!(!store
        .spend_state(NOW)
        .await
        .unwrap()
        .ledger
        .unwrap()
        .closed_for_commitments());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn distributed_counters_fail_closed_when_a_declared_partition_is_missing() {
    let (client, name, store, _) = fixture(10_000, true).await;
    let held = reserve(&store, "held", 100).await.unwrap();
    store
        .model_budget
        .delete_one(doc! {"_id": counters::key(held.start, 0)})
        .await
        .unwrap();
    assert!(matches!(
        store.spend_state(NOW).await,
        Err(BudgetError::Unavailable)
    ));
    assert!(matches!(
        reserve(&store, "later", 1).await,
        Err(BudgetError::Unavailable)
    ));
    client.database(&name).drop().await.unwrap();
}
