use super::*;
use layer2_core::sandbox::store::SandboxStore;
use layer3_adapter::SandboxSpec;
use tokio::time::Instant;

#[path = "reconciliation_sweep_fixtures.rs"]
mod fixtures;
use fixtures::{sandbox, setup, Failure, Output};

#[path = "reconciliation_sweep_policy_tests.rs"]
mod policy_tests;

async fn complete(reconciler: &SandboxReconciler) -> usize {
    for attempt in 1..=30 {
        if reconciler.run_due(SystemTime::now()).await.unwrap() {
            return attempt;
        }
    }
    panic!("finite healthy work must finish");
}

#[tokio::test(start_paused = true)]
async fn slow_sweep_refreshes_bindings_without_replaying_maintenance_candidates() {
    let (store, reconciler) = setup(8, Duration::from_secs(2)).await;
    let reconciler = reconciler.with_runtime_version("fixture-image".into());
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let span = tracing::info_span!("caller", private_context = "private fixture content");
    let _entered = span.enter();
    let budget = crate::background::BackgroundBudget::new();
    let work = crate::background::BackgroundWork::new(
        budget,
        "sandbox_reconciliation",
        Duration::from_secs(30),
        Duration::ZERO,
    );
    let started = Instant::now();
    let mut attempts = 0;
    loop {
        let before = Instant::now();
        let result = work
            .run_resumable(|| reconciler.run_due(SystemTime::now()))
            .await
            .unwrap();
        assert!(before.elapsed() < Duration::from_secs(30));
        attempts += 1;
        if result == Some(true) {
            break;
        }
        assert!(attempts < 30);
    }
    assert!(started.elapsed() > Duration::from_secs(30));
    assert!(attempts > 1);
    let expected = (0..8)
        .map(|i| sandbox(i).id.to_string())
        .collect::<Vec<_>>();
    let authority_passes = store.keys("authority").len() / 4;
    assert!(authority_passes >= 2);
    // Initial/refresh passes and runtime-update reads each visit every row.
    for id in &expected {
        assert_eq!(
            store.keys("get").iter().filter(|key| *key == id).count(),
            authority_passes + 1
        );
    }
    assert_eq!(store.keys("pause"), expected);
    for cursor in ["start", "sbx_001", "sbx_003", "sbx_005"] {
        assert_eq!(
            store
                .keys("authority")
                .iter()
                .filter(|key| key.as_str() == cursor)
                .count(),
            authority_passes
        );
    }
    assert_eq!(
        store.keys("candidates"),
        ["start", "sbx_001", "sbx_003", "sbx_005"]
    );
    assert!(reconciler.progress.lock().await.is_none());
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    let sweep = logs
        .lines()
        .filter(|line| line.contains("sandbox maintenance"))
        .collect::<Vec<_>>();
    assert!(sweep.iter().any(|line| line.contains("reason=\"yielded\"")));
    for phase in [
        "authority",
        "bindings",
        "runtime_cleanup",
        "claim_cleanup",
        "home_cleanup",
        "profiles",
        "runtime_updates",
        "idle_pause",
    ] {
        assert!(
            sweep
                .iter()
                .any(|line| line.contains(&format!("phase=\"{phase}\""))
                    && line.contains("elapsed_ms=")),
            "{phase}"
        );
    }
    assert_eq!(
        sweep
            .iter()
            .filter(|line| line.contains("sweep completed"))
            .count(),
        1
    );
    assert!(sweep
        .iter()
        .all(|line| !line.contains("sbx_") && !line.contains("private fixture content")));
    assert!(logs
        .lines()
        .any(|line| line.contains("reason=\"yielded\"") && line.contains("completed=0")));
    assert!(logs
        .lines()
        .any(|line| line.contains("reason=\"complete\"") && line.contains("completed=1")));
    let decisions = logs
        .lines()
        .filter(|line| line.contains("automatic sandbox pause decision"))
        .collect::<Vec<_>>();
    assert_eq!(decisions.len(), 8);
    assert!(decisions.iter().all(|line| line.contains("sandbox_id=sbx_")
        && line.contains("outcome=NotEligible")
        && line.contains("threshold_seconds=900")
        && !line.contains("private fixture content")));
}

#[tokio::test(start_paused = true)]
async fn item_deadline_resumes_the_same_candidate_without_repeating_completed_items() {
    let (store, reconciler) = setup(4, Duration::ZERO).await;
    store.fail("pause", "sbx_001", Failure::Deadline);
    let started = Instant::now();
    assert!(reconciler.run_due(SystemTime::now()).await.is_err());
    assert_eq!(started.elapsed(), OPERATION);
    assert_eq!(store.keys("pause"), ["sbx_000", "sbx_001"]);
    assert_eq!(
        reconciler
            .progress
            .lock()
            .await
            .as_ref()
            .unwrap()
            .sweep
            .phase,
        Phase::IdlePause
    );
    complete(&reconciler).await;
    assert_eq!(
        store.keys("pause"),
        ["sbx_000", "sbx_001", "sbx_001", "sbx_002", "sbx_003"]
    );
    assert_eq!(store.keys("authority"), ["start", "sbx_001"].repeat(2));
    assert_eq!(store.keys("candidates"), ["start", "sbx_001"]);
}

#[tokio::test(start_paused = true)]
async fn outer_cancellation_retains_the_interrupted_item_and_emits_attempt_telemetry() {
    let (store, reconciler) = setup(3, Duration::ZERO).await;
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    store.fail("pause", "sbx_001", Failure::Deadline);
    assert!(tokio::time::timeout(
        Duration::from_secs(1),
        reconciler.run_due(SystemTime::now())
    )
    .await
    .is_err());
    complete(&reconciler).await;
    assert_eq!(
        store.keys("pause"),
        ["sbx_000", "sbx_001", "sbx_001", "sbx_002"]
    );
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.lines()
            .any(|line| line.contains("reason=\"cancelled\"")
                && line.contains("phase=\"idle_pause\""))
    );
}

#[tokio::test(start_paused = true)]
async fn partial_authority_never_reclaims_and_retries_only_the_unfinished_page() {
    for failure in [Failure::Deadline, Failure::Unavailable, Failure::Corrupt] {
        let (store, reconciler) = setup(4, Duration::ZERO).await;
        let orphan = reconciler
            .adapter
            .provision(&SandboxSpec {
                sandbox_id: "sbx_orphan".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        store.fail("authority", "sbx_001", failure);
        assert!(reconciler.run_due(SystemTime::now()).await.is_err());
        assert!(store.keys("runtime_fence").is_empty());
        assert!(store.keys("get").is_empty());
        assert!(reconciler
            .adapter
            .runtime_inventory()
            .await
            .unwrap()
            .iter()
            .any(|row| row.handle == orphan));
        complete(&reconciler).await;
        assert_eq!(store.keys("authority"), ["start", "sbx_001", "sbx_001"]);
        assert!(reconciler
            .adapter
            .runtime_inventory()
            .await
            .unwrap()
            .is_empty());
        assert_eq!(store.keys("pause").len(), 4);
    }
}

#[tokio::test(start_paused = true)]
async fn resumed_cleanup_rereads_pool_and_owned_authority_before_reclaim() {
    let (store, reconciler) = setup(1, Duration::ZERO).await;
    let mut claimed = sandbox(9);
    claimed.lifecycle.runtime_generation_high_water = 1;
    let runtime = reconciler
        .adapter
        .provision(&SandboxSpec {
            sandbox_id: claimed.id.to_string(),
            ..Default::default()
        })
        .await
        .unwrap();
    store.fail("get", claimed.id.as_str(), Failure::Deadline);
    assert!(reconciler.run_due(SystemTime::now()).await.is_err());
    // Simulate an atomic warm claim publishing owned authority after the first
    // pool fence but before the resumed cleanup item.
    store.inner.create_sandbox(claimed.clone()).await.unwrap();
    complete(&reconciler).await;
    assert_eq!(
        store.keys("runtime_fence"),
        [claimed.id.as_str(), claimed.id.as_str()]
    );
    assert!(reconciler
        .adapter
        .runtime_inventory()
        .await
        .unwrap()
        .iter()
        .any(|row| row.handle == runtime));
}

#[tokio::test(start_paused = true)]
async fn malformed_authority_page_is_rejected_atomically() {
    use layer2_core::sandbox::store::Page;
    let mut authority = Authority::default();
    assert!(!authority
        .accept(Page {
            items: vec![sandbox(0)],
            next: Some("sbx_000".into())
        })
        .unwrap());
    assert!(authority
        .accept(Page {
            items: vec![sandbox(2), sandbox(1)],
            next: None
        })
        .is_err());
    assert_eq!(authority.items.len(), 1);
    assert_eq!(authority.request.cursor.as_deref(), Some("sbx_000"));
    assert!(authority
        .accept(Page {
            items: vec![],
            next: Some("sbx_000".into())
        })
        .is_err());
    assert!(authority
        .accept(Page {
            items: vec![sandbox(1)],
            next: Some("sbx_099".into())
        })
        .is_err());
    assert!(authority
        .accept(Page {
            items: vec![sandbox(1)],
            next: None
        })
        .unwrap());
}

#[tokio::test(start_paused = true)]
async fn budget_deferral_preserves_an_existing_sweep_without_polling_it() {
    use std::sync::Arc;
    let (store, reconciler) = setup(8, Duration::from_secs(2)).await;
    assert!(!reconciler.run_due(SystemTime::now()).await.unwrap());
    let before = store.calls.lock().unwrap().clone();
    let phase = reconciler
        .progress
        .lock()
        .await
        .as_ref()
        .unwrap()
        .sweep
        .phase;
    let budget = crate::background::BackgroundBudget::new();
    let release = Arc::new(tokio::sync::Notify::new());
    let mut holders = Vec::new();
    for name in ["first", "second"] {
        let work = crate::background::BackgroundWork::new(
            budget.clone(),
            name,
            Duration::from_secs(30),
            Duration::ZERO,
        );
        let release = release.clone();
        holders.push(tokio::spawn(async move {
            work.run(|| async {
                release.notified().await;
                Ok(())
            })
            .await
        }));
    }
    tokio::task::yield_now().await;
    let work = crate::background::BackgroundWork::new(
        budget,
        "sandbox_reconciliation",
        Duration::from_secs(30),
        Duration::ZERO,
    );
    assert_eq!(
        work.run_resumable(|| reconciler.run_due(SystemTime::now()))
            .await
            .unwrap(),
        None
    );
    assert_eq!(*store.calls.lock().unwrap(), before);
    assert_eq!(
        reconciler
            .progress
            .lock()
            .await
            .as_ref()
            .unwrap()
            .sweep
            .phase,
        phase
    );
    release.notify_waiters();
    for holder in holders {
        holder.await.unwrap().unwrap();
    }
    complete(&reconciler).await;
    assert_eq!(
        store.keys("pause"),
        (0..8)
            .map(|i| sandbox(i).id.to_string())
            .collect::<Vec<_>>()
    );
}

#[path = "reconciliation_home_inventory_tests.rs"]
mod home_inventory;

#[path = "reconciliation_binding_tests.rs"]
mod binding_tests;

#[path = "reconciliation_binding_observation_tests.rs"]
mod binding_observation;

#[path = "reconciliation_schedule_tests.rs"]
mod schedule_tests;

#[path = "reconciliation_opportunity_tests.rs"]
mod opportunity_tests;

#[path = "reconciliation_diagnostics_tests.rs"]
mod diagnostics_tests;

#[path = "reconciliation_recovery_tests.rs"]
mod recovery_tests;

#[path = "reconciliation_supervision_tests.rs"]
mod supervision_tests;

#[path = "reconciliation_cadence_tests.rs"]
mod cadence_tests;
