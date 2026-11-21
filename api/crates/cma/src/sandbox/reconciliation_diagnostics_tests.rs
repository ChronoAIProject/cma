use super::*;
use layer3_adapter::binding_diagnostics::{self as diagnostics, Outcome, Stage};

#[tokio::test(start_paused = true)]
async fn stage_diagnostics_keep_results_errors_and_cancellation_identity_free() {
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let span = tracing::info_span!(
        "private caller",
        sandbox_id = "secret-sandbox",
        error = "private error"
    );
    let _entered = span.enter();
    for stage in [
        Stage::DurableRead,
        Stage::Adoption,
        Stage::NativeIdentity,
        Stage::NativeReadiness,
        Stage::NativeAssignment,
        Stage::TransportWait,
        Stage::Status,
        Stage::FinalLossObservation,
        Stage::InterruptionObservation,
        Stage::Transaction,
    ] {
        for (error, outcome) in [
            (None, "complete"),
            (
                Some(layer3_adapter::AdapterError::Unavailable(
                    "private error".into(),
                )),
                "unavailable",
            ),
            (
                Some(layer3_adapter::AdapterError::Rejected(
                    "secret-sandbox".into(),
                )),
                "refused",
            ),
            (
                Some(layer3_adapter::AdapterError::AtCapacity(
                    "private error".into(),
                )),
                "capacity",
            ),
            (
                Some(layer3_adapter::AdapterError::OperationIdentityConflict(
                    "private error".into(),
                )),
                "conflict",
            ),
        ] {
            let expected = error.map_or(Ok(17), Err);
            let result = diagnostics::scope(
                Instant::now() + Duration::from_secs(20),
                diagnostics::measure(
                    stage,
                    async {
                        tokio::time::sleep(Duration::from_millis(123)).await;
                        expected.clone()
                    },
                    diagnostics::adapter_outcome,
                ),
            )
            .await;
            assert_eq!(result, expected);
            let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
            let last = logs.lines().last().unwrap();
            assert!(last.contains(&format!("outcome=\"{outcome}\"")));
            assert!(last.contains("elapsed_ms=123"));
        }
        for (limit, expected) in [(7, "cancelled"), (20, "deadline")] {
            let pending = diagnostics::scope(
                Instant::now() + Duration::from_secs(20),
                diagnostics::measure(stage, std::future::pending::<()>(), |_| Outcome::Complete),
            );
            assert!(tokio::time::timeout(Duration::from_secs(limit), pending)
                .await
                .is_err());
            let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
            let last = logs.lines().last().unwrap();
            assert!(last.contains(&format!("outcome=\"{expected}\"")));
            assert!(last.contains(&format!("elapsed_ms={}", limit * 1000)));
        }
    }
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert_eq!(logs.lines().count(), 70);
    assert!(!logs.contains("secret-sandbox"));
    assert!(!logs.contains("private error"));
    assert!(!logs.contains("private caller"));
    let before = output.0.lock().unwrap().len();
    diagnostics::measure(Stage::Status, async { 9 }, |_| Outcome::Complete).await;
    assert_eq!(
        output.0.lock().unwrap().len(),
        before,
        "ordinary adapter calls do not add binding diagnostics"
    );
}

#[tokio::test(start_paused = true)]
async fn binding_store_failures_report_the_stage_without_copying_raw_errors() {
    for (failure, expected) in [
        (Failure::Unavailable, "unavailable"),
        (Failure::Corrupt, "corrupt"),
    ] {
        let (store, reconciler) = setup(2, Duration::ZERO).await;
        let output = Output::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(output.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        store.fail("get", "sbx_000", failure);
        assert!(reconciler.run_due(SystemTime::now()).await.unwrap());
        let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        let stages: Vec<_> = logs
            .lines()
            .filter(|line| line.contains("sandbox binding stage completed"))
            .collect();
        assert!(stages
            .iter()
            .any(|line| line.contains("stage=\"durable_read\"")
                && line.contains(&format!("outcome=\"{expected}\""))));
        assert!(stages
            .iter()
            .all(|line| !line.contains("sbx_") && !line.contains("fixture")));
        assert!(reconciler.binding_holds.contains("sbx_000"));
        assert_eq!(store.keys("pause"), ["sbx_001"]);
    }
}
