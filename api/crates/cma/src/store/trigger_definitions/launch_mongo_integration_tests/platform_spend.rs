//! Launch admission reads the platform admission state after the visitor quota.
use super::*;
use layer2_core::budget::{
    BudgetError, BudgetPolicy, BudgetSettings, Closure, ClosureReason, CloudObservation,
    CloudSpendPolicy, PlatformBudgetPolicy, PlatformPeriodLedger,
};
use layer2_core::trigger_domain::LaunchAuthorityClass;

fn platform() -> PlatformBudgetPolicy {
    PlatformBudgetPolicy {
        period_cap: u64::from(u32::MAX),
        period_start: "2026-01-01T00:00:00Z".into(),
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

/// Enabled budgets with one visitor Launch per day, and an observed period row
/// carrying `closures`.
async fn configure(seed: &Seeded, revision: u64, closures: &[ClosureReason]) {
    let settings_store =
        crate::store::MongoPlatformSettingsStore::open(&seed.client, &seed.database)
            .await
            .unwrap();
    use layer2_core::PlatformSettingsStore;
    let mut settings = layer2_core::PlatformSettings::default();
    settings.limits.visitor_max_launches_per_day = 1;
    settings.model_budget = BudgetSettings {
        enabled: true,
        platform: Some(platform()),
        default_policy: Some(BudgetPolicy {
            owner_day_limit: 255,
            turn_limit: 255,
            max_calls_per_turn: 255,
            day_timezone: "UTC".into(),
            input_token_units: 1,
            output_token_units: 1,
            max_unobserved_units: 1,
            usage_timeout_seconds: 60,
        }),
        owners: vec![],
        request_models: vec![],
    };
    settings_store.replace(revision, &settings).await.unwrap();
    let mut ledger = PlatformPeriodLedger::open(&platform(), revision + 1).unwrap();
    ledger
        .record_cloud(CloudObservation {
            observed_through: ledger.period_start,
            units: 0,
            recorded_at: ledger.period_start,
            settings_revision: revision + 1,
        })
        .unwrap();
    ledger.closures = closures
        .iter()
        .map(|reason| Closure {
            reason: *reason,
            since: ledger.period_start,
            settings_revision: revision + 1,
            committed_at_entry: 0,
        })
        .collect();
    let key = crate::store::budget::period_key(ledger.period_start);
    seed.client
        .database(&seed.database)
        .collection::<Document>("model_budget_reservations")
        .replace_one(
            doc! {"_id": &key},
            doc! {"_id": &key,
            "document_schema_version": crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION,
            "record": mongodb::bson::serialize_to_bson(&ledger).unwrap()},
        )
        .upsert(true)
        .await
        .unwrap();
}

fn visitor(trigger: &PublicTrigger) -> LaunchAdmission {
    let mut candidate = launch(trigger, 'a', 'b');
    candidate.authority = LaunchAuthorityClass::Visitor;
    LaunchAdmission {
        launch: candidate,
        trigger: trigger.clone(),
        events: vec![LaunchEventKind::Created],
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn launch_admission_is_refused_under_spend_stop_after_visitor_quota() {
    let seed = seeded("platform_launch").await;
    let trigger = published(&seed, "platform_launch").await;
    let database = seed.client.database(&seed.database);
    configure(
        &seed,
        0,
        &[ClosureReason::SpendWarning, ClosureReason::SpendStop],
    )
    .await;
    assert!(matches!(
        seed.store.admit_launch(visitor(&trigger)).await,
        Err(LaunchStoreError::Budget(BudgetError::PlatformExhausted))
    ));
    // The refusal aborted the transaction that had consumed the allowance.
    for collection in [LAUNCHES, "visitor_launch_days"] {
        assert_eq!(
            database
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0,
            "{collection}"
        );
    }
    configure(&seed, 1, &[ClosureReason::MeteringUnavailable]).await;
    assert!(matches!(
        seed.store.admit_launch(visitor(&trigger)).await,
        Err(LaunchStoreError::Budget(BudgetError::Unavailable))
    ));
    // Warning closes registration only; an authenticated visitor still launches.
    configure(&seed, 2, &[ClosureReason::SpendWarning]).await;
    assert!(matches!(
        seed.store.admit_launch(visitor(&trigger)).await,
        Ok(LaunchAdmissionResult::Created(_))
    ));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn operator_stop_refuses_fresh_launches_with_metering_off_but_replays_an_admitted_launch() {
    use layer2_core::admission::AdmissionControlStore;
    let seed = seeded("operator_launch").await;
    let trigger = published(&seed, "operator_launch").await;
    let control = crate::store::admission_control::MongoAdmissionControlStore::open(
        &seed.client,
        &seed.database,
    )
    .await
    .unwrap();
    let actor = SubjectId::new("operator");
    control
        .change(0, &actor, "contain new work", true, 100)
        .await
        .unwrap();
    assert!(matches!(
        seed.store.admit_launch(visitor(&trigger)).await,
        Err(LaunchStoreError::AdmissionPaused)
    ));
    let database = seed.client.database(&seed.database);
    for collection in [LAUNCHES, "visitor_launch_days"] {
        assert_eq!(
            database
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    control
        .change(1, &actor, "reopen", false, 101)
        .await
        .unwrap();
    let admitted = seed.store.admit_launch(visitor(&trigger)).await.unwrap();
    let LaunchAdmissionResult::Created(original) = admitted else {
        panic!("new launch expected")
    };
    control
        .change(2, &actor, "second incident", true, 102)
        .await
        .unwrap();
    let replay = seed.store.admit_launch(visitor(&trigger)).await.unwrap();
    assert!(
        matches!(replay, LaunchAdmissionResult::Existing(ref saved) if saved.id == original.id)
    );
    database.drop().await.unwrap();
}
