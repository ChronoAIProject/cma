use super::*;

#[tokio::test(start_paused = true)]
async fn local_hold_metric_reports_failure_backoff_and_clear_only_when_enabled() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    for enabled in [false, true] {
        let (store, reconciler) = setup(1, Duration::ZERO).await;
        let reconciler = reconciler.with_binding_hold_metrics(enabled);
        store.fail("get", "sbx_000", Failure::Unavailable);
        complete(&reconciler).await;
        assert!(reconciler.binding_is_held(&SandboxId("sbx_000".into())));
        if !enabled {
            assert!(!handle.render().contains("cma_sandbox_binding_holds_local"));
            continue;
        }
        assert!(handle
            .render()
            .lines()
            .any(|line| line == "cma_sandbox_binding_holds_local 1"));
        tokio::time::advance(Duration::from_secs(29)).await;
        complete(&reconciler).await;
        assert_eq!(store.keys("get"), ["sbx_000"]);
        assert!(handle
            .render()
            .lines()
            .any(|line| line == "cma_sandbox_binding_holds_local 1"));
        tokio::time::advance(Duration::from_secs(1)).await;
        complete(&reconciler).await;
        assert!(!reconciler.binding_is_held(&SandboxId("sbx_000".into())));
        assert!(handle
            .render()
            .lines()
            .any(|line| line == "cma_sandbox_binding_holds_local 0"));
    }
}

#[tokio::test(start_paused = true)]
async fn raced_recovery_is_reported_separately_from_cancellation() {
    let (_store, reconciler) = setup(1, Duration::ZERO).await;
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let row = sandbox(0);
    let attempt = reconciler.binding_holds.begin(&row, &[]).unwrap();
    attempt.raced();
    drop(reconciler.binding_holds.report_on_drop());
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("reason=\"raced\""));
    assert!(!logs.contains("reason=\"cancelled\""));
}

#[tokio::test(start_paused = true)]
async fn binding_failures_back_off_across_sweeps_and_success_resets_attempts() {
    let (store, reconciler) = setup(2, Duration::ZERO).await;
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let span = tracing::info_span!("caller", private_context = "private conversation");
    let _entered = span.enter();
    for (attempt, delay) in [30, 60, 120, 240, 300, 300].into_iter().enumerate() {
        store.fail("get", "sbx_000", Failure::Unavailable);
        complete(&reconciler).await;
        assert!(reconciler.binding_holds.contains("sbx_000"));
        assert_eq!(
            store
                .keys("get")
                .iter()
                .filter(|id| *id == "sbx_000")
                .count(),
            attempt + 1
        );
        assert!(!store.keys("pause").contains(&"sbx_000".into()));
        assert!(store.keys("pause").contains(&"sbx_001".into()));
        tokio::time::advance(Duration::from_secs(delay - 1)).await;
        complete(&reconciler).await;
        assert_eq!(
            store
                .keys("get")
                .iter()
                .filter(|id| *id == "sbx_000")
                .count(),
            attempt + 1
        );
        tokio::time::advance(Duration::from_secs(1)).await;
    }
    complete(&reconciler).await;
    assert!(!reconciler.binding_holds.contains("sbx_000"));
    store.fail("get", "sbx_000", Failure::Corrupt);
    complete(&reconciler).await;
    tokio::time::advance(Duration::from_secs(30)).await;
    complete(&reconciler).await;
    assert!(!reconciler.binding_holds.contains("sbx_000"));
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    let holds = logs
        .lines()
        .filter(|line| line.contains("sandbox binding hold"))
        .collect::<Vec<_>>();
    assert!(holds
        .iter()
        .any(|line| line.contains("reason=\"unavailable\"") && line.contains("held_bindings=1")));
    assert!(holds.iter().any(|line| line.contains("reason=\"corrupt\"")));
    assert!(holds.iter().all(|line| !line.contains("sbx_")
        && !line.contains("private conversation")
        && !line.contains("fixture")));
}

#[tokio::test(start_paused = true)]
async fn timeout_and_cancellation_hold_the_row_but_release_the_cursor() {
    for cancel in [false, true] {
        let (store, reconciler) = setup(2, Duration::ZERO).await;
        store.fail("get", "sbx_000", Failure::Deadline);
        let started = Instant::now();
        if cancel {
            assert!(tokio::time::timeout(
                Duration::from_secs(1),
                reconciler.run_due(SystemTime::now())
            )
            .await
            .is_err());
            assert_eq!(started.elapsed(), Duration::from_secs(1));
        }
        complete(&reconciler).await;
        if !cancel {
            assert_eq!(started.elapsed(), OPERATION);
        }
        assert!(reconciler.binding_holds.contains("sbx_000"));
        assert_eq!(store.keys("get"), ["sbx_000", "sbx_001"]);
        assert_eq!(store.keys("pause"), ["sbx_001"]);
        // The interval begins at completion, not at the start of slow I/O.
        tokio::time::advance(Duration::from_secs(29)).await;
        complete(&reconciler).await;
        assert!(reconciler.binding_holds.contains("sbx_000"));
        tokio::time::advance(Duration::from_secs(1)).await;
        complete(&reconciler).await;
        assert!(!reconciler.binding_holds.contains("sbx_000"));
    }
}

#[tokio::test(start_paused = true)]
async fn held_binding_retains_foreign_handle_and_home_owner_while_other_orphans_are_reclaimed() {
    let (store, reconciler) = setup(2, Duration::ZERO).await;
    let runtime = reconciler
        .adapter
        .provision(&SandboxSpec {
            sandbox_id: "sbx_foreign".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let orphan = reconciler
        .adapter
        .provision(&SandboxSpec {
            sandbox_id: "sbx_independent_orphan".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut held = sandbox(0);
    held.runtime = Some(layer2_core::sandbox::RuntimeBinding {
        runtime_id: runtime.0.clone(),
        runtime_version: "fixture".into(),
        provision_generation: Some(1),
        boot_id: None,
        bound_at: held.updated_at.clone(),
    });
    held.lifecycle.runtime_generation_high_water = 1;
    held.revision += 1;
    store.inner.save_sandbox(1, held.clone()).await.unwrap();
    complete(&reconciler).await;
    assert!(reconciler.binding_holds.contains("sbx_000"));
    assert!(reconciler.binding_holds.contains("sbx_foreign"));
    assert_eq!(store.inner.get_sandbox(&held.id).await.unwrap(), Some(held));
    assert!(!store.keys("runtime_fence").contains(&"sbx_foreign".into()));
    let remaining = reconciler.adapter.runtime_inventory().await.unwrap();
    assert!(remaining.iter().any(|r| r.handle == runtime));
    assert!(!remaining.iter().any(|r| r.handle == orphan));
    assert_eq!(store.keys("pause"), ["sbx_001"]);
}

#[tokio::test(start_paused = true)]
async fn slow_bindings_still_yield_inside_the_total_budget() {
    let (store, reconciler) = setup(10, Duration::from_secs(3)).await;
    let start = Instant::now();
    assert!(!reconciler.run_due(SystemTime::now()).await.unwrap());
    assert!(start.elapsed() <= SLICE + OPERATION);
    // The authority is complete before any binding can be held or skipped.
    assert_eq!(store.keys("authority").len(), 5);
    complete(&reconciler).await;
}

#[tokio::test(start_paused = true)]
async fn each_ambiguous_binding_protects_its_runtime_owner() {
    let (store, reconciler) = setup(2, Duration::ZERO).await;
    let mut expected = Vec::new();
    for (index, owner) in [(0, "sbx_001"), (1, "sbx_foreign")] {
        let handle = reconciler
            .adapter
            .provision(&SandboxSpec {
                sandbox_id: owner.into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let mut row = sandbox(index);
        row.runtime = Some(layer2_core::sandbox::RuntimeBinding {
            runtime_id: handle.0.clone(),
            runtime_version: "fixture".into(),
            provision_generation: Some(1),
            boot_id: None,
            bound_at: row.updated_at.clone(),
        });
        row.lifecycle.runtime_generation_high_water = 1;
        row.revision += 1;
        store.inner.save_sandbox(1, row.clone()).await.unwrap();
        expected.push(row);
    }
    complete(&reconciler).await;
    assert_eq!(store.keys("get"), ["sbx_000", "sbx_001"]);
    assert!(reconciler.binding_holds.contains("sbx_foreign"));
    assert_eq!(
        reconciler.adapter.runtime_inventory().await.unwrap().len(),
        2
    );
    assert!(store.keys("runtime_fence").is_empty());
    assert!(store.keys("pause").is_empty());
    for row in expected {
        assert_eq!(store.inner.get_sandbox(&row.id).await.unwrap(), Some(row));
    }
}
