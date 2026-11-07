use super::*;

// Derived from the reviewer's two 100-minute starvation probes. Assert bounded
// recovery while the tail remains faulty, without granting orphan reclamation.
#[tokio::test(start_paused = true)]
async fn due_binding_recovers_within_two_invocations_despite_a_persistent_tail_failure() {
    for (phase, key, expected_phase) in [
        ("runtime_fence", "sbx_orphan_probe", Phase::Runtimes),
        ("pause", "sbx_001", Phase::IdlePause),
    ] {
        let (store, reconciler) = setup(2, Duration::ZERO).await;
        let orphan = if phase == "runtime_fence" {
            Some(
                reconciler
                    .adapter
                    .provision(&SandboxSpec {
                        sandbox_id: key.into(),
                        ..Default::default()
                    })
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        store.fail("get", "sbx_000", Failure::Deadline);
        assert!(tokio::time::timeout(
            Duration::from_secs(1),
            reconciler.run_due(SystemTime::now())
        )
        .await
        .is_err());
        assert!(reconciler.binding_holds.contains("sbx_000"));
        for invocation in 1..=20 {
            tokio::time::advance(Duration::from_secs(300)).await;
            store.fail(phase, key, Failure::Unavailable);
            assert!(reconciler.run_due(SystemTime::now()).await.is_err());
            assert_eq!(
                reconciler
                    .progress
                    .lock()
                    .await
                    .as_ref()
                    .unwrap()
                    .sweep
                    .phase,
                expected_phase
            );
            if invocation >= 2 {
                assert!(!reconciler.binding_holds.contains("sbx_000"));
                assert_eq!(
                    store
                        .keys("get")
                        .iter()
                        .filter(|id| *id == "sbx_000")
                        .count(),
                    invocation
                );
            }
            if let Some(orphan) = &orphan {
                assert!(reconciler
                    .adapter
                    .runtime_inventory()
                    .await
                    .unwrap()
                    .iter()
                    .any(|r| &r.handle == orphan));
            }
        }
        complete(&reconciler).await;
        assert!(!reconciler.binding_holds.contains("sbx_000"));
    }
}

#[tokio::test(start_paused = true)]
async fn incomplete_refresh_authority_keeps_the_uncertain_cleanup_item_retained() {
    for failure in [Failure::Corrupt, Failure::Unavailable, Failure::Deadline] {
        let (store, reconciler) = setup(4, Duration::ZERO).await;
        let orphan = reconciler
            .adapter
            .provision(&SandboxSpec {
                sandbox_id: "sbx_orphan".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        store.fail("runtime_fence", "sbx_orphan", Failure::Unavailable);
        assert!(reconciler.run_due(SystemTime::now()).await.is_err());
        store.fail("authority", "sbx_001", failure);
        assert!(reconciler.run_due(SystemTime::now()).await.is_err());
        assert_eq!(store.keys("runtime_fence"), ["sbx_orphan"]);
        assert!(reconciler
            .adapter
            .runtime_inventory()
            .await
            .unwrap()
            .iter()
            .any(|r| r.handle == orphan));
        store.fail("runtime_fence", "sbx_orphan", Failure::Unavailable);
        assert!(reconciler.run_due(SystemTime::now()).await.is_err());
        assert_eq!(
            store.keys("authority"),
            ["start", "sbx_001", "start", "sbx_001", "sbx_001"]
        );
        assert_eq!(store.keys("runtime_fence"), ["sbx_orphan", "sbx_orphan"]);
    }
}

#[tokio::test(start_paused = true)]
async fn cancellation_during_refresh_retains_both_cursors_and_binding_protection() {
    let (store, reconciler) = setup(2, Duration::ZERO).await;
    store.fail("pause", "sbx_001", Failure::Unavailable);
    assert!(reconciler.run_due(SystemTime::now()).await.is_err());
    store.fail("get", "sbx_000", Failure::Deadline);
    assert!(tokio::time::timeout(
        Duration::from_secs(1),
        reconciler.run_due(SystemTime::now())
    )
    .await
    .is_err());
    {
        let progress = reconciler.progress.lock().await;
        let progress = progress.as_ref().unwrap();
        assert_eq!(progress.sweep.phase, Phase::IdlePause);
        assert_eq!(
            progress.binding_refresh.as_ref().unwrap().binding_cursor,
            Some(sandbox(0).id)
        );
    }
    assert!(reconciler.binding_holds.contains("sbx_000"));
    tokio::time::advance(Duration::from_secs(30)).await;
    for _ in 0..2 {
        store.fail("pause", "sbx_001", Failure::Unavailable);
        assert!(reconciler.run_due(SystemTime::now()).await.is_err());
    }
    assert!(!reconciler.binding_holds.contains("sbx_000"));
    assert_eq!(
        store
            .keys("pause")
            .iter()
            .filter(|id| *id == "sbx_000")
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn a_new_hold_during_refresh_blocks_each_retained_admission_boundary() {
    for phase in [Phase::Profiles, Phase::RuntimeUpdates, Phase::IdlePause] {
        let (store, reconciler) = setup(1, Duration::ZERO).await;
        let reconciler = reconciler.with_runtime_version("fixture-image".into());
        // Retain a candidate between its profile/update/pause stages.
        *reconciler.progress.lock().await = Some(Progress {
            sweep: Sweep {
                phase,
                candidates: VecDeque::from([sandbox(0).id]),
                candidates_finished: true,
                ..Default::default()
            },
            refresh_required: true,
            ..Default::default()
        });
        store.fail("get", "sbx_000", Failure::Unavailable);
        complete(&reconciler).await;
        assert!(reconciler.binding_holds.contains("sbx_000"));
        assert_eq!(store.keys("get"), ["sbx_000"]);
        assert!(store.keys("pause").is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn a_refresh_that_exhausts_the_slice_still_gives_the_tail_its_next_opportunity() {
    let (store, reconciler) = setup(8, Duration::from_secs(2)).await;
    *reconciler.progress.lock().await = Some(Progress {
        sweep: Sweep {
            phase: Phase::IdlePause,
            candidates: VecDeque::from([sandbox(7).id]),
            candidates_finished: true,
            ..Default::default()
        },
        refresh_required: true,
        ..Default::default()
    });
    for invocation in 1..=3 {
        let started = Instant::now();
        let done = reconciler.run_due(SystemTime::now()).await.unwrap();
        assert!(started.elapsed() <= SLICE + OPERATION);
        if done {
            break;
        }
        assert!(invocation < 3);
    }
    assert_eq!(store.keys("pause"), ["sbx_007"]);
    assert_eq!(
        store.keys("authority"),
        ["start", "sbx_001", "sbx_003", "sbx_005"]
    );
}
