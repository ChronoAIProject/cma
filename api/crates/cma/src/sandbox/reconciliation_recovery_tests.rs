use super::*;
use layer2_core::sandbox::*;
use std::sync::{Arc, Mutex};

#[path = "reconciliation_recovery_fixtures.rs"]
mod support;
use support::{candidate, recovery_setup, Verify};

#[path = "reconciliation_recovery_opportunity_tests.rs"]
mod opportunity_tests;

// Preserve PR1's absent/held/attempted accounting regression on the separate lane.
#[tokio::test(start_paused = true)]
async fn recovery_progress_counts_attempted_candidates_only() {
    for (present, held, expected) in [(false, false, 0), (true, true, 0), (true, false, 1)] {
        let (store, reconciler, verifier) = recovery_setup(usize::from(present), Some(0)).await;
        let id = sandbox(0).id;
        let before = store.inner.get_sandbox(&id).await.unwrap();
        if held {
            drop(
                reconciler
                    .binding_holds
                    .begin(before.as_ref().unwrap(), &[])
                    .unwrap(),
            );
        }
        let output = Output::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(output.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let mut lane = super::super::super::recovery::Schedule::default();
        lane.run(&reconciler).await.unwrap();
        assert!(verifier.calls.lock().unwrap().is_empty());
        assert!(store.keys("get").is_empty());
        assert!(!String::from_utf8(output.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .any(|line| line.contains("stage=Opportunity") && line.contains("attempted=1")));
        lane.run(&reconciler).await.unwrap();
        let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        assert_eq!(
            logs.lines()
                .filter(|line| line.contains("stage=Opportunity") && line.contains("attempted=1"))
                .count(),
            expected
        );
        assert_eq!(verifier.calls.lock().unwrap().len(), expected);
        assert_eq!(store.keys("get").len(), expected);
        assert!(store.keys("recovery_admit").is_empty());
        assert_eq!(reconciler.binding_holds.contains(id.as_str()), held);
        assert_eq!(store.inner.get_sandbox(&id).await.unwrap(), before);
        // Discovery cannot populate or complete the ordinary cleanup authority.
        assert!(reconciler.progress.lock().await.is_none());
    }
}

#[tokio::test(start_paused = true)]
async fn denied_background_slot_does_not_poll_or_advance_a_due_recovery_turn() {
    let (store, reconciler, verifier) = recovery_setup(2, Some(0)).await;
    assert!(reconciler.run_due(SystemTime::now()).await.unwrap());
    let before = store.calls.lock().unwrap().clone();
    let budget = crate::background::BackgroundBudget::new();
    let release = Arc::new(tokio::sync::Notify::new());
    let mut holders = Vec::new();
    for name in ["one", "two"] {
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
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
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
    assert!(verifier.calls.lock().unwrap().is_empty());
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("shared_budget_denied") && logs.contains("attempted=0"));
    release.notify_waiters();
    for holder in holders {
        holder.await.unwrap().unwrap();
    }
    assert_eq!(
        work.run_resumable(|| reconciler.run_due(SystemTime::now()))
            .await
            .unwrap(),
        Some(false)
    );
    assert!(verifier.calls.lock().unwrap().is_empty());
    // Ordinary work still separates discovery from the reserved proof turn.
    work.run_resumable(|| reconciler.run_due(SystemTime::now()))
        .await
        .unwrap();
    work.run_resumable(|| reconciler.run_due(SystemTime::now()))
        .await
        .unwrap();
    assert_eq!(*verifier.calls.lock().unwrap(), [sandbox(0).id.clone()]);
}

#[tokio::test(start_paused = true)]
async fn cancellation_after_proof_does_not_reuse_proof_or_pin_selection() {
    let (store, reconciler, verifier) = recovery_setup(2, None).await;
    let before = store
        .inner
        .get_sandbox(&sandbox(0).id)
        .await
        .unwrap()
        .unwrap();
    let mut lane = super::super::super::recovery::Schedule::default();
    lane.run(&reconciler).await.unwrap();
    store.fail("recovery_admit", "sbx_000", Failure::Deadline);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), lane.run(&reconciler))
            .await
            .is_err()
    );
    assert_eq!(
        store
            .inner
            .get_sandbox(&sandbox(0).id)
            .await
            .unwrap()
            .unwrap(),
        before
    );
    lane.run(&reconciler).await.unwrap();
    lane.run(&reconciler).await.unwrap();
    lane.run(&reconciler).await.unwrap();
    lane.run(&reconciler).await.unwrap();
    assert_eq!(
        *verifier.calls.lock().unwrap(),
        [
            sandbox(0).id.clone(),
            sandbox(1).id.clone(),
            sandbox(0).id.clone()
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn discovery_yields_at_slice_bound_and_resumes_from_validated_page() {
    let (store, reconciler) = setup(22, Duration::from_secs(2)).await;
    store.create_sandbox(candidate(22)).await.unwrap();
    let verifier = Arc::new(Verify {
        calls: Mutex::default(),
        refuse: None,
    });
    let service = Arc::new(
        SandboxRuntimeUpdateService::new(store.clone(), "fixture-image".into())
            .with_unpublished_recovery(verifier.clone()),
    );
    let reconciler = reconciler.with_failed_runtime_recovery(service);
    let mut lane = super::super::super::recovery::Schedule::default();
    let started = Instant::now();
    lane.run(&reconciler).await.unwrap();
    assert_eq!(started.elapsed(), Duration::from_secs(20));
    assert!(verifier.calls.lock().unwrap().is_empty());
    lane.run(&reconciler).await.unwrap();
    assert!(verifier.calls.lock().unwrap().is_empty());
    lane.run(&reconciler).await.unwrap();
    assert_eq!(*verifier.calls.lock().unwrap(), [sandbox(22).id.clone()]);
    assert_eq!(
        store
            .keys("authority")
            .iter()
            .filter(|key| *key == "start")
            .count(),
        1
    );
    assert!(store.keys("runtime_fence").is_empty());
    assert!(reconciler.progress.lock().await.is_none());
}

#[tokio::test(start_paused = true)]
async fn six_candidates_progress_past_refusing_first_with_an_unending_cleanup_tail() {
    let (store, reconciler, verifier) = recovery_setup(7, Some(0)).await;
    let before = store
        .inner
        .get_sandbox(&sandbox(0).id)
        .await
        .unwrap()
        .unwrap();
    let orphan = reconciler
        .adapter
        .provision(&SandboxSpec {
            sandbox_id: "sbx_orphan".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    for index in 0..7 {
        for dispatch in [false, true] {
            store.fail("runtime_fence", "sbx_orphan", Failure::Unavailable);
            assert!(reconciler.run_due(SystemTime::now()).await.is_err());
            assert!(!reconciler.run_due(SystemTime::now()).await.unwrap());
            assert_eq!(
                verifier.calls.lock().unwrap().len(),
                index + usize::from(dispatch)
            );
        }
        assert!(reconciler
            .adapter
            .runtime_inventory()
            .await
            .unwrap()
            .iter()
            .any(|r| r.handle == orphan));
    }
    assert_eq!(
        *verifier.calls.lock().unwrap(),
        (0..7).map(|i| sandbox(i).id).collect::<Vec<_>>()
    );
    assert_eq!(
        store.keys("recovery_admit"),
        (1..7)
            .map(|i| sandbox(i).id.to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        store
            .inner
            .get_sandbox(&sandbox(0).id)
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(
        reconciler
            .progress
            .lock()
            .await
            .as_ref()
            .unwrap()
            .sweep
            .phase,
        Phase::Runtimes
    );
    assert!(store.keys("authority").iter().any(|key| key == "sbx_005"));
    // Another refused attempt still rotates; it cannot monopolize binding work.
    for _ in 0..2 {
        store.fail("runtime_fence", "sbx_orphan", Failure::Unavailable);
        assert!(reconciler.run_due(SystemTime::now()).await.is_err());
        reconciler.run_due(SystemTime::now()).await.unwrap();
    }
    assert_eq!(verifier.calls.lock().unwrap().last(), Some(&sandbox(0).id));
}

#[tokio::test(start_paused = true)]
async fn held_candidates_advance_without_admission_or_retiring_any_hold() {
    let (store, reconciler, verifier) = recovery_setup(3, None).await;
    let held = store
        .inner
        .get_sandbox(&sandbox(0).id)
        .await
        .unwrap()
        .unwrap();
    drop(reconciler.binding_holds.begin(&held, &[]).unwrap());
    // The discovery lane never completes/retains the ordinary authority scan.
    let mut lane = super::super::super::recovery::Schedule::default();
    lane.run(&reconciler).await.unwrap();
    lane.run(&reconciler).await.unwrap();
    assert_eq!(*verifier.calls.lock().unwrap(), [sandbox(1).id.clone()]);
    assert!(reconciler.binding_holds.contains(held.id.as_str()));
    assert_eq!(
        store.inner.get_sandbox(&held.id).await.unwrap().unwrap(),
        held
    );
    lane.run(&reconciler).await.unwrap();
    lane.run(&reconciler).await.unwrap();
    assert_eq!(
        *verifier.calls.lock().unwrap(),
        [sandbox(1).id.clone(), sandbox(2).id.clone()]
    );
}

#[tokio::test(start_paused = true)]
async fn timeout_advances_and_restart_rereads_without_rearming_admitted_work() {
    let (store, reconciler, verifier) = recovery_setup(3, None).await;
    let mut lane = super::super::super::recovery::Schedule::default();
    store.fail("recovery_snapshot", "sbx_000", Failure::Deadline);
    lane.run(&reconciler).await.unwrap();
    let started = Instant::now();
    lane.run(&reconciler).await.unwrap();
    assert_eq!(started.elapsed(), Duration::from_secs(20));
    lane.run(&reconciler).await.unwrap();
    lane.run(&reconciler).await.unwrap();
    assert_eq!(*verifier.calls.lock().unwrap(), [sandbox(1).id.clone()]);
    // Restart loses scheduling hints only; fresh discovery excludes committed work.
    let mut restarted = super::super::super::recovery::Schedule::default();
    restarted.run(&reconciler).await.unwrap();
    restarted.run(&reconciler).await.unwrap();
    restarted.run(&reconciler).await.unwrap();
    restarted.run(&reconciler).await.unwrap();
    assert_eq!(
        store.keys("recovery_admit"),
        ["sbx_001", "sbx_000", "sbx_002"]
    );
}

#[tokio::test(start_paused = true)]
async fn incomplete_discovery_never_releases_holds_or_authorizes_orphan_cleanup() {
    let (store, reconciler, _) = recovery_setup(0, None).await;
    for i in 0..4 {
        store.create_sandbox(sandbox(i)).await.unwrap();
    }
    let held = sandbox(10);
    drop(reconciler.binding_holds.begin(&held, &[]).unwrap());
    let orphan = reconciler
        .adapter
        .provision(&SandboxSpec {
            sandbox_id: "sbx_orphan".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut lane = super::super::super::recovery::Schedule::default();
    store.fail("authority", "sbx_001", Failure::Corrupt);
    assert!(lane.run(&reconciler).await.is_err());
    assert!(reconciler.binding_holds.contains(held.id.as_str()));
    assert!(store.keys("runtime_fence").is_empty());
    assert!(reconciler
        .adapter
        .runtime_inventory()
        .await
        .unwrap()
        .iter()
        .any(|r| r.handle == orphan));
    assert!(reconciler.progress.lock().await.is_none());
    lane.run(&reconciler).await.unwrap();
    assert_eq!(store.keys("authority"), ["start", "sbx_001", "sbx_001"]);
    assert!(reconciler.binding_holds.contains(held.id.as_str()));
}

#[tokio::test(start_paused = true)]
async fn the_independent_recovery_lock_consumes_the_reserved_binding_deadline() {
    use super::opportunity_tests::{bindings, reserve};

    let (store, reconciler) = setup(1, Duration::ZERO).await;
    let verifier = Arc::new(Verify {
        calls: Mutex::default(),
        refuse: None,
    });
    let service = Arc::new(
        SandboxRuntimeUpdateService::new(store.clone(), "fixture-image".into())
            .with_unpublished_recovery(verifier.clone()),
    );
    let reconciler = reconciler
        .with_failed_runtime_recovery(service)
        .with_runtime_loss_bounded_proof(true);
    reserve(&reconciler, 0, "deadline").await;
    bindings(&reconciler, 1).await;
    let guard = reconciler.recovery_schedule.lock().await;
    let release = async {
        tokio::time::sleep(Duration::from_secs(25)).await;
        drop(guard);
    };
    let started = Instant::now();
    let (_, result) = tokio::join!(release, reconciler.run_due(SystemTime::now()));
    assert!(!result.unwrap());
    assert_eq!(started.elapsed(), Duration::from_secs(25));
    assert!(store.keys("get").is_empty());
    assert!(store.keys("pause").is_empty());
    assert!(reconciler.binding_holds.contains("sbx_000"));
    assert!(reconciler
        .progress
        .lock()
        .await
        .as_ref()
        .unwrap()
        .sweep
        .binding_cursor
        .is_none());

    // The independent turn keeps its existing priority. The following binding
    // opportunity sees the original held item exactly once.
    assert!(!reconciler.run_due(SystemTime::now()).await.unwrap());
    assert!(store.keys("get").is_empty());
    assert!(!reconciler.run_due(SystemTime::now()).await.unwrap());
    assert_eq!(store.keys("get"), ["sbx_000"]);
    assert!(verifier.calls.lock().unwrap().is_empty());
}

/// Toggleable platform spend hint for the recovery lane (#1757).
struct SpendStop(std::sync::atomic::AtomicBool);

#[async_trait::async_trait]
impl crate::sandbox::reconciliation::platform::PlatformSpendSweep for SpendStop {
    async fn evaluate_platform_spend(
        &self,
        _: u64,
    ) -> Result<Option<bool>, layer2_core::budget::BudgetError> {
        Ok(Some(self.0.load(std::sync::atomic::Ordering::SeqCst)))
    }
}

// A failed platform pause must not be reopened as Active by automatic
// recovery while SpendStop holds. The candidate keeps its eligibility and
// recovers exactly as before once the stop lifts (#1757, decision 26).
#[tokio::test(start_paused = true)]
async fn spend_stop_defers_automatic_unpublished_recovery_until_it_lifts() {
    let (store, reconciler, verifier) = recovery_setup(1, None).await;
    let stop = Arc::new(SpendStop(std::sync::atomic::AtomicBool::new(true)));
    let reconciler = reconciler.with_platform_spend(stop.clone());
    let id = sandbox(0).id;
    let before = store.inner.get_sandbox(&id).await.unwrap().unwrap();
    let stopped_at = SystemTime::now();
    // One ordinary sweep turn, then the recovery turn, which is deferred.
    for _ in 0..2 {
        assert!(reconciler.run_due(stopped_at).await.unwrap());
    }
    assert!(verifier.calls.lock().unwrap().is_empty());
    assert!(store.keys("recovery_admit").is_empty());
    let held = store.inner.get_sandbox(&id).await.unwrap().unwrap();
    assert_eq!(held.status, SandboxStatus::Failed);
    assert!(held.unpublished_recovery_candidate());
    assert!(!held.lifecycle.unpublished_recovery_attempted);
    assert_eq!(
        held.lifecycle.unpublished_recovery,
        before.lifecycle.unpublished_recovery
    );

    // After the lift the same lane resumes: discovery, then one proof. This
    // fixture lists Failed rows as maintenance candidates (Mongo does not), so
    // a further sweep turn would hold the candidate's binding; skip those
    // sweep turns to observe only the recovery turns of `run_due`.
    stop.0.store(false, std::sync::atomic::Ordering::SeqCst);
    let lifted_at = stopped_at + Duration::from_secs(16);
    for _ in 0..2 {
        assert!(!reconciler.recovery_schedule.lock().await.take_turn());
        assert!(!reconciler.run_due(lifted_at).await.unwrap());
    }
    assert_eq!(*verifier.calls.lock().unwrap(), std::slice::from_ref(&id));
    assert_eq!(store.keys("recovery_admit").len(), 1);
    assert!(
        store
            .inner
            .get_sandbox(&id)
            .await
            .unwrap()
            .unwrap()
            .lifecycle
            .unpublished_recovery_attempted
    );
}

#[path = "reconciliation_release_source_tests.rs"]
mod release_source_tests;
