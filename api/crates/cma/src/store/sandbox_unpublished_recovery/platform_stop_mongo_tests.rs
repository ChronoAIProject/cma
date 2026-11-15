//! A failed pause must not authorize a new runtime commitment under STOP.
use super::*;
use layer2_core::admission::AdmissionControlStore;
use layer2_core::budget::{
    BudgetError, BudgetPolicy, BudgetSettings, CloudObservation, CloudSpendPolicy,
    PlatformBudgetPolicy, PlatformPeriodLedger,
};
use layer2_core::PlatformSettings;

async fn spend_stop(store: &MongoSandboxStore) -> PlatformBudgetPolicy {
    let policy = PlatformBudgetPolicy {
        period_cap: 10_000,
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
    };
    let settings = PlatformSettings {
        model_budget: BudgetSettings {
            enabled: true,
            platform: Some(policy.clone()),
            default_policy: Some(BudgetPolicy {
                owner_day_limit: 1_000,
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
    settings.model_budget.validate().unwrap();
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
    let now = layer2_core::seconds_from_rfc3339(NOW).unwrap();
    let mut ledger = PlatformPeriodLedger::open(&policy, 1).unwrap();
    ledger
        .record_cloud(CloudObservation {
            observed_through: now,
            units: policy.period_cap,
            recorded_at: now,
            settings_revision: 1,
        })
        .unwrap();
    ledger.evaluate(&policy, 1, now).unwrap();
    store
        .model_budget
        .insert_one(doc! {
            "_id": crate::store::budget::period_key(ledger.period_start),
            "document_schema_version": 1_i32,
            "record": mongodb::bson::serialize_to_bson(&ledger).unwrap(),
        })
        .await
        .unwrap();
    policy
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn failed_platform_pause_cannot_admit_runtime_recovery_until_lift() {
    let (client, name, store) = database().await;
    let mut request = fixture(&store, "sbx_failed_platform_pause").await;
    let mut failed = request.snapshot.sandbox.clone();
    failed.status = SandboxStatus::Active;
    failed.failure = None;
    failed
        .begin_pause(NOW.into(), Some(PLATFORM_SPEND_PAUSE_NOTICE.into()))
        .unwrap();
    let fence = failed
        .work
        .claim("pause-worker".into(), NOW, "2026-09-05T01:00:00Z".into())
        .unwrap();
    failed
        .fail_lifecycle(
            "runtime suspension unavailable".into(),
            "pause-worker",
            fence,
            NOW.into(),
        )
        .unwrap();
    assert_eq!(failed.status, SandboxStatus::Failed);
    assert!(
        failed.runtime.is_some(),
        "failure precedes runtime suspension"
    );
    store
        .sandboxes
        .replace_one(
            doc! {"_id": failed.id.as_str()},
            crate::store::sandboxes::sandbox_document(&failed, 0).unwrap(),
        )
        .await
        .unwrap();
    request.snapshot = store
        .unpublished_runtime_recovery_snapshot(&failed.id)
        .await
        .unwrap();
    request.automatic = true;
    request.target_runtime_version = failed.runtime.as_ref().unwrap().runtime_version.clone();
    let policy = spend_stop(&store).await;
    crate::store::admission_control::MongoAdmissionControlStore::open(&client, &name)
        .await
        .unwrap()
        .change(
            0,
            &layer2_core::SubjectId::new("operator"),
            "contain new work",
            true,
            100,
        )
        .await
        .unwrap();

    assert_eq!(
        store
            .admit_unpublished_runtime_recovery(request.clone())
            .await,
        Err(SandboxStoreError::Budget(BudgetError::PlatformExhausted))
    );
    assert_eq!(
        store.get_sandbox(&failed.id).await.unwrap().unwrap(),
        failed
    );
    assert_eq!(
        store
            .get_agent(&request.snapshot.agents[0].id)
            .await
            .unwrap()
            .unwrap(),
        request.snapshot.agents[0]
    );
    assert_eq!(
        store.sandbox_events.count_documents(doc! {}).await.unwrap(),
        0
    );

    // Correct the evidence and authorize the lift with a newer policy revision.
    store
        .budget_settings
        .update_one(doc! {"_id": "platform"}, doc! {"$inc": {"revision": 1_i64}})
        .await
        .unwrap();
    store
        .model_budget
        .update_one(
            doc! {"_id": crate::store::budget::period_key(policy.period_bounds().unwrap().0)},
            doc! {"$set": {"record.cloud.units": 0_i64}, "$inc": {"record.revision": 1_i64}},
        )
        .await
        .unwrap();
    store
        .evaluate_period(layer2_core::seconds_from_rfc3339(NOW).unwrap())
        .await
        .unwrap();
    // The operator stop remains active: this recovers existing admitted work.
    // Only the lifted spend closure was blocking its runtime liability.
    let recovered = store
        .admit_unpublished_runtime_recovery(request.clone())
        .await
        .unwrap();
    assert_eq!(recovered.status, SandboxStatus::Rotating);
    assert_eq!(recovered.lifecycle.home, failed.lifecycle.home);
    assert_eq!(
        store
            .get_agent(&request.snapshot.agents[0].id)
            .await
            .unwrap()
            .unwrap(),
        request.snapshot.agents[0],
        "recovery admission never replays queued input"
    );
    client.database(&name).drop().await.unwrap();
}
