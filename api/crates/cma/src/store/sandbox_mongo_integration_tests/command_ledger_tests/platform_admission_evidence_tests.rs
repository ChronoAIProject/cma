//! Admission must fail closed even when the evaluator has not run.
use super::budget_tests::{configure, configure_with_platform, observe, open_platform, period_row};
use super::*;
use layer2_core::budget::{BudgetError, ClosureReason, PlatformSpendLedger};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn admission_derives_missing_stale_and_finished_metering_without_evaluator() {
    let (client, name) = database("platform_admission_evidence").await;
    let (store, agent, _) = fixture(&name, &client).await;
    let mut platform = open_platform();
    platform.cloud.as_mut().unwrap().max_observation_age_seconds = 60;
    configure_with_platform(&store, &agent.owner, platform.clone(), 1).await;
    let now = layer2_core::seconds_from_rfc3339(NOW).unwrap();
    let key = crate::store::budget::period_key(platform.period_bounds().unwrap().0);

    for evidence in ["missing_period", "missing_observation", "stale", "finished"] {
        if evidence != "missing_period" {
            observe(&store, &platform, 0, now).await;
        }
        if evidence == "missing_observation" {
            store
                .model_budget
                .update_one(
                    doc! {"_id": &key},
                    doc! {"$set": {"record.cloud": mongodb::bson::Bson::Null}},
                )
                .await
                .unwrap();
        }
        let at = match evidence {
            "stale" => now + 61,
            "finished" => {
                let end = platform.period_bounds().unwrap().1;
                observe(&store, &platform, 0, end).await;
                end
            }
            _ => now,
        };
        let before = period_row(&store, &platform).await;
        let state = store.admission_state(at).await.unwrap();
        assert!(state.configured);
        assert_eq!(
            state.commitment_refusal(),
            Some(BudgetError::Unavailable),
            "{evidence}"
        );
        assert!(state.closed_for_registration(), "{evidence}");
        assert_eq!(state.closures[0].reason, ClosureReason::MeteringUnavailable);
        // Admission must not depend on successfully persisting an evaluation.
        assert_eq!(period_row(&store, &platform).await, before);
    }

    observe(&store, &platform, 0, now).await;
    let state = store.admission_state(now).await.unwrap();
    assert!(!state.closed_for_commitments());
    assert!(!state.closed_for_registration());
    configure(&store, &agent.owner).await;
    assert!(!store.admission_state(u64::MAX).await.unwrap().configured);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn admission_derives_thresholds_but_does_not_lift_persisted_closures() {
    let (client, name) = database("platform_admission_thresholds").await;
    let (store, agent, _) = fixture(&name, &client).await;
    let platform = open_platform();
    configure_with_platform(&store, &agent.owner, platform.clone(), 1).await;
    let now = layer2_core::seconds_from_rfc3339(NOW).unwrap();
    let levels = platform.levels().unwrap();
    observe(&store, &platform, levels.warning, now).await;
    let state = store.admission_state(now).await.unwrap();
    assert!(state.closed_for_registration());
    assert!(!state.closed_for_commitments());
    observe(&store, &platform, levels.disposable, now).await;
    assert_eq!(
        store
            .admission_state(now)
            .await
            .unwrap()
            .commitment_refusal(),
        Some(BudgetError::PlatformExhausted)
    );
    store.evaluate_period(now).await.unwrap();
    observe(&store, &platform, 0, now).await;
    configure_with_platform(&store, &agent.owner, platform.clone(), 2).await;
    assert_eq!(
        store
            .admission_state(now)
            .await
            .unwrap()
            .commitment_refusal(),
        Some(BudgetError::PlatformExhausted)
    );
    store.evaluate_period(now).await.unwrap();
    assert!(!store
        .admission_state(now)
        .await
        .unwrap()
        .closed_for_registration());
    client.database(&name).drop().await.unwrap();
}
