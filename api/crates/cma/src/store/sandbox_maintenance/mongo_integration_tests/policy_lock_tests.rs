use super::*;
use mongodb::event::EventHandler;
use std::{sync::Mutex, time::Duration};

#[path = "policy_lock_timing.rs"]
mod timing;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn global_settings_remain_writable_until_candidate_local_persistence_finishes() {
    let (admin, name, replica) = database().await;
    let (_, policy) = policy_tests::enable(&admin, &name).await;
    let (sandbox, _) = fixture(&replica, "sbx_policy_lock").await;
    let timing = Arc::new(Mutex::new(timing::Timing::default()));
    let observed = timing.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        observed.lock().unwrap().observe(event);
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let gate = Arc::new(super::super::admission_test_gate::Gate::default());
    let request = SandboxInactivityPauseRequest {
        auto_pause_enabled: true,
        expected_policy_revision: Some(policy.revision),
        ..pause(&sandbox.id)
    };
    let mut probe = admin.start_session().await.unwrap();
    start_transaction(&mut probe).await.unwrap();
    let candidate_gate = gate.clone();
    let mut candidate = tokio::spawn(async move {
        candidate_gate
            .scope(store.schedule_inactivity_pause(request))
            .await
    });
    let reached = tokio::time::timeout(Duration::from_secs(10), gate.reached()).await;
    // This independent transaction probes the actual shared row while the
    // candidate has completed its Sandbox, Agent, usage and event writes.
    let probe_result = if reached.is_ok() {
        Some(
            replica
                .budget_settings
                .update_one(
                    doc! {"_id":"platform"},
                    doc! {"$inc":{"inactivity_admission_fence":1_i64}},
                )
                .session(&mut probe)
                .await,
        )
    } else {
        None
    };
    let probe_aborted = probe.abort_transaction().await;
    gate.release();
    let outcome = match tokio::time::timeout(Duration::from_secs(10), &mut candidate).await {
        Ok(outcome) => outcome,
        Err(_) => {
            candidate.abort();
            candidate.await
        }
    };
    let elapsed = timing.lock().unwrap().elapsed;
    let fence = replica
        .budget_settings
        .find_one(doc! {"_id":"platform"})
        .await
        .unwrap()
        .unwrap()
        .get_i64("inactivity_admission_fence")
        .unwrap_or(0);
    admin.database(&name).drop().await.unwrap();

    assert!(
        reached.is_ok(),
        "candidate did not finish local persistence"
    );
    let result_label = match &outcome {
        Ok(Ok(value)) => format!("{value:?}"),
        Ok(Err(error)) => super::contention_tests::diagnostics::classify_error(error).into(),
        Err(_) => "task_failed".into(),
    };
    assert!(
        matches!(outcome, Ok(Ok(SandboxMaintenanceOutcome::Scheduled))),
        "candidate_result={result_label}"
    );
    assert_eq!(fence, 1, "the probe must leave no committed fence");
    eprintln!(
        "global_fence_to_commit_ms={:.3}",
        elapsed
            .expect("successful policy write and commit must be observed")
            .as_secs_f64()
            * 1000.0
    );
    let probe_transient = probe_result
        .as_ref()
        .is_some_and(|result| matches!(result, Err(error) if is_transient(error)));
    assert!(
        matches!(probe_result, Some(Ok(result)) if result.matched_count == 1),
        "settings must remain writable through local persistence; probe_transient={probe_transient}"
    );
    assert!(probe_aborted.is_ok(), "successful probe must abort");
}
