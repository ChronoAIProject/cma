use super::*;

#[path = "reconciliation_prelude_tests.rs"]
mod prelude;

#[tokio::test(start_paused = true)]
async fn a_reserved_retry_keeps_parent_revision_fences_after_loss_observation() {
    let (store, reconciler) = setup(2, Duration::ZERO).await;
    let mut original = sandbox(0);
    original.runtime = Some(layer2_core::sandbox::RuntimeBinding {
        runtime_id: "absent-original-runtime".into(),
        runtime_version: "original-image".into(),
        provision_generation: Some(1),
        boot_id: None,
        bound_at: original.updated_at.clone(),
    });
    original.lifecycle.runtime_generation_high_water = 1;
    original.revision += 1;
    store.inner.save_sandbox(1, original.clone()).await.unwrap();
    reserve(&reconciler, 0, "deadline").await;
    let mut newer = original.clone();
    newer.revision += 1;
    newer.name = "newer parent authority".into();
    *store.loss_race.lock().unwrap() = Some(newer.clone());
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    complete(&reconciler).await;
    assert_eq!(
        store.inner.get_sandbox(&original.id).await.unwrap(),
        Some(newer)
    );
    assert!(reconciler.binding_holds.contains("sbx_000"));
    assert_eq!(store.keys("pause"), ["sbx_001"]);
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(logs
        .lines()
        .any(|line| line.contains("stage=\"transaction\"")
            && line.contains("outcome=\"superseded\"")));
}

#[tokio::test(start_paused = true)]
async fn a_lost_transaction_reply_is_reconciled_by_fresh_state_without_replaying_loss() {
    let (store, reconciler) = setup(2, Duration::ZERO).await;
    let mut original = sandbox(0);
    original.runtime = Some(layer2_core::sandbox::RuntimeBinding {
        runtime_id: "absent-original-runtime".into(),
        runtime_version: "original-image".into(),
        provision_generation: Some(1),
        boot_id: None,
        bound_at: original.updated_at.clone(),
    });
    original.lifecycle.runtime_generation_high_water = 1;
    original.revision += 1;
    store.inner.save_sandbox(1, original.clone()).await.unwrap();
    store.fail("loss_reply", "sbx_000", Failure::Deadline);
    assert!(reconciler.run_due(SystemTime::now()).await.unwrap());
    assert!(reconciler.binding_holds.contains("sbx_000"));
    let committed = store
        .inner
        .get_sandbox(&original.id)
        .await
        .unwrap()
        .unwrap();
    assert!(committed.runtime.is_none());
    assert!(committed.revision > original.revision);
    assert_eq!(store.keys("loss"), ["sbx_000"]);
    tokio::time::advance(Duration::from_secs(30)).await;
    complete(&reconciler).await;
    assert!(!reconciler.binding_holds.contains("sbx_000"));
    assert_eq!(store.keys("loss"), ["sbx_000"]);
    assert_eq!(
        store.inner.get_sandbox(&original.id).await.unwrap(),
        Some(committed)
    );
}

async fn reserve(reconciler: &SandboxReconciler, index: usize, reason: &'static str) {
    let row = sandbox(index);
    drop(reconciler.binding_holds.begin(&row, &[]).unwrap());
    reconciler.binding_holds.failed(&row.id, reason);
    tokio::time::advance(Duration::from_secs(30)).await;
}

async fn bindings(reconciler: &SandboxReconciler, count: usize) {
    let mut sweep = Sweep {
        phase: Phase::Bindings,
        ..Default::default()
    };
    for index in 0..count {
        let row = sandbox(index);
        sweep.authority.items.insert(row.id.clone(), row);
    }
    *reconciler.progress.lock().await = Some(Progress {
        sweep,
        ..Default::default()
    });
}

#[tokio::test(start_paused = true)]
async fn only_due_deadline_or_cancelled_holds_reserve_an_empty_slice() {
    for reason in [
        "deadline",
        "cancelled",
        "unavailable",
        "corrupt",
        "conflict",
        "raced",
    ] {
        let (store, reconciler) = setup(2, Duration::ZERO).await;
        let row = sandbox(0);
        drop(reconciler.binding_holds.begin(&row, &[]).unwrap());
        reconciler.binding_holds.failed(&row.id, reason);
        assert!(!reconciler.binding_holds.reserved_retry_due(&row.id, false));
        tokio::time::advance(Duration::from_secs(29)).await;
        complete(&reconciler).await;
        assert_eq!(store.keys("get"), ["sbx_001"]);
        tokio::time::advance(Duration::from_secs(1)).await;
        let reserved = matches!(reason, "deadline" | "cancelled");
        assert_eq!(
            reconciler.binding_holds.reserved_retry_due(&row.id, false),
            reserved
        );
        bindings(&reconciler, 2).await;
        store.fail("get", "sbx_000", Failure::Delay(Duration::from_secs(6)));
        let started = Instant::now();
        reconciler.run_due(SystemTime::now()).await.unwrap();
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(if reserved { 6 } else { 5 })
        );
        assert_eq!(reconciler.binding_holds.contains("sbx_000"), !reserved);
        complete(&reconciler).await;
        assert!(store.keys("pause").contains(&"sbx_001".into()));
    }
}

#[tokio::test(start_paused = true)]
async fn a_late_reserved_item_yields_without_advancing_its_cursor_or_extending_backoff() {
    let (store, reconciler) = setup(6, Duration::from_secs(4)).await;
    reserve(&reconciler, 4, "deadline").await;
    bindings(&reconciler, 6).await;
    let started = Instant::now();
    assert!(!reconciler.run_due(SystemTime::now()).await.unwrap());
    assert_eq!(started.elapsed(), Duration::from_secs(16));
    assert_eq!(
        store.keys("get"),
        ["sbx_000", "sbx_001", "sbx_002", "sbx_003"]
    );
    assert_eq!(
        reconciler
            .progress
            .lock()
            .await
            .as_ref()
            .unwrap()
            .sweep
            .binding_cursor,
        Some(sandbox(3).id)
    );
    assert!(reconciler
        .binding_holds
        .reserved_retry_due(&sandbox(4).id, false));
    store.fail("get", "sbx_004", Failure::Delay(Duration::from_secs(19)));
    let started = Instant::now();
    assert!(!reconciler.run_due(SystemTime::now()).await.unwrap());
    assert_eq!(started.elapsed(), Duration::from_secs(19));
    assert!(!reconciler.binding_holds.contains("sbx_004"));
    assert_eq!(
        store
            .keys("get")
            .iter()
            .filter(|id| *id == "sbx_004")
            .count(),
        1
    );
    complete(&reconciler).await;
    assert!(store.keys("pause").contains(&"sbx_005".into()));
}

#[tokio::test(start_paused = true)]
async fn healthy_first_attempts_batch_without_receiving_the_reserved_budget() {
    let (store, reconciler) = setup(3, Duration::ZERO).await;
    assert!(reconciler.run_due(SystemTime::now()).await.unwrap());
    assert_eq!(store.keys("get"), ["sbx_000", "sbx_001", "sbx_002"]);
    store.fail("get", "sbx_000", Failure::Delay(Duration::from_secs(6)));
    let started = Instant::now();
    assert!(reconciler.run_due(SystemTime::now()).await.unwrap());
    assert_eq!(started.elapsed(), OPERATION);
    assert!(reconciler.binding_holds.contains("sbx_000"));
    assert!(store
        .keys("pause")
        .ends_with(&["sbx_001".into(), "sbx_002".into()]));
}

#[tokio::test(start_paused = true)]
async fn reserved_deadline_and_outer_cancellation_release_permits_and_allow_later_progress() {
    for cancel in [false, true] {
        let (store, reconciler) = setup(2, Duration::ZERO).await;
        reserve(&reconciler, 0, "deadline").await;
        bindings(&reconciler, 2).await;
        let output = Output::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(output.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let work = crate::background::BackgroundWork::new(
            crate::background::BackgroundBudget::new(),
            "sandbox_reconciliation",
            Duration::from_secs(30),
            Duration::ZERO,
        );
        store.fail("get", "sbx_000", Failure::Delay(Duration::from_secs(21)));
        let started = Instant::now();
        let run = work.run_resumable(|| reconciler.run_due(SystemTime::now()));
        if cancel {
            assert!(tokio::time::timeout(Duration::from_secs(7), run)
                .await
                .is_err());
        } else {
            assert_eq!(run.await.unwrap(), Some(false));
        }
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(if cancel { 7 } else { 20 })
        );
        assert!(reconciler.binding_holds.contains("sbx_000"));
        let retry_at = Instant::now() + Duration::from_secs(60);
        assert_eq!(
            reconciler
                .progress
                .lock()
                .await
                .as_ref()
                .unwrap()
                .sweep
                .binding_cursor,
            Some(sandbox(0).id)
        );
        assert_eq!(
            work.run_resumable(|| reconciler.run_due(SystemTime::now()))
                .await
                .unwrap(),
            Some(true)
        );
        assert_eq!(store.keys("get"), ["sbx_000", "sbx_001"]);
        assert_eq!(store.keys("pause"), ["sbx_001"]);
        tokio::time::advance(
            retry_at.saturating_duration_since(Instant::now()) - Duration::from_secs(1),
        )
        .await;
        assert!(!reconciler
            .binding_holds
            .reserved_retry_due(&sandbox(0).id, false));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(reconciler
            .binding_holds
            .reserved_retry_due(&sandbox(0).id, false));
        let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        let outcome = if cancel { "cancelled" } else { "deadline" };
        assert!(logs
            .lines()
            .any(|line| line.contains("stage=\"durable_read\"")
                && line.contains(&format!("outcome=\"{outcome}\""))));
    }
}

#[tokio::test(start_paused = true)]
async fn bounded_recovery_reserves_races_and_caps_an_empty_slice_at_twenty_five_seconds() {
    for delay in [24, 26] {
        let (store, reconciler) = setup(2, Duration::ZERO).await;
        let reconciler = reconciler.with_runtime_loss_bounded_proof(true);
        reserve(&reconciler, 0, "raced").await;
        assert!(reconciler
            .binding_holds
            .reserved_retry_due(&sandbox(0).id, true));
        assert!(!reconciler
            .binding_holds
            .reserved_retry_due(&sandbox(0).id, false));
        bindings(&reconciler, 2).await;
        store.fail("get", "sbx_000", Failure::Delay(Duration::from_secs(delay)));
        let started = Instant::now();
        assert!(!reconciler.run_due(SystemTime::now()).await.unwrap());
        assert_eq!(started.elapsed(), Duration::from_secs(delay.min(25)));
        assert_eq!(reconciler.binding_holds.contains("sbx_000"), delay > 25);
        complete(&reconciler).await;
        assert!(store.keys("pause").contains(&"sbx_001".into()));
    }
}

#[tokio::test(start_paused = true)]
async fn cancelled_loss_reports_one_identified_outcome_and_keeps_binding_protection() {
    let (store, reconciler) = setup(1, Duration::ZERO).await;
    let mut original = sandbox(0);
    original.runtime = Some(layer2_core::sandbox::RuntimeBinding {
        runtime_id: "absent-original-runtime".into(),
        runtime_version: "image".into(),
        provision_generation: Some(1),
        boot_id: None,
        bound_at: original.updated_at.clone(),
    });
    original.lifecycle.runtime_generation_high_water = 1;
    original.revision += 1;
    store.inner.save_sandbox(1, original).await.unwrap();
    store.fail("loss_reply", "sbx_000", Failure::Deadline);
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    reconciler.run_due(SystemTime::now()).await.unwrap();
    assert!(reconciler.binding_holds.contains("sbx_000"));
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    let results = logs
        .lines()
        .filter(|line| line.contains("runtime loss reconciliation outcome"))
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 1);
    assert!(results[0].contains("sandbox_id=sbx_000"));
    assert!(results[0].contains("outcome=\"cancelled\""));
}
