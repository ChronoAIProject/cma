use super::*;

#[tokio::test(start_paused = true)]
async fn native_create_retry_observation_failure_keeps_ordinary_delay_even_after_pending() {
    for prior_pending in [false, true] {
        for stalled in [false, true] {
            let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
            let created = h.create().await;
            let pools = pools(&h, &[]);
            pools.pending_claim.store(true, Ordering::SeqCst);
            pools.observation.set_delay(Duration::from_millis(100));
            let fail_read = || {
                if stalled {
                    pools.stall_observe.store(true, Ordering::SeqCst);
                } else {
                    *pools.observe_error.lock().unwrap() =
                        Some(AdapterError::Unavailable("transport".into()));
                }
            };
            if !prior_pending {
                fail_read();
            }
            let worker = cold_worker(&h, pools.clone(), true);
            let now = start_time(&created);
            let id = created.id.clone();
            let job = tokio::spawn(async move { worker.run_one(&id, now).await });
            if prior_pending {
                pools.observed.notified().await;
                fail_read();
            } else {
                tokio::task::yield_now().await;
            }
            let original = h.stored(&created.id).await.lifecycle.home;
            assert!(original
                .as_ref()
                .unwrap()
                .native_pool
                .as_ref()
                .unwrap()
                .claim
                .is_some());
            assert!(job.await.unwrap().unwrap());
            let stored = h.stored(&created.id).await;
            assert_eq!(
                stored.work.ready_at,
                rfc3339_at(now + Duration::from_secs(30))
            );
            assert_eq!(stored.lifecycle.home, original);
            assert_eq!(stored.work.attempts, 1);
            assert_eq!(call_count(&pools, "claim"), 1);
            assert_eq!(call_count(&pools, "adopt"), 0);
            assert_eq!(call_count(&pools, "reclaim"), 0);
            pools.stall_observe.store(false, Ordering::SeqCst);
            pools.pending_claim.store(false, Ordering::SeqCst);
            cold_worker(&h, pools.clone(), true)
                .run_one(&created.id, now + Duration::from_secs(30))
                .await
                .unwrap();
            let recovered = h.stored(&created.id).await;
            let claim = &recovered
                .lifecycle
                .home
                .as_ref()
                .unwrap()
                .native_pool
                .as_ref()
                .unwrap()
                .claim;
            assert_eq!(
                claim.as_ref().unwrap().uid,
                original.unwrap().native_pool.unwrap().claim.unwrap().uid
            );
            assert!(recovered.runtime.is_some());
            assert_eq!(call_count(&pools, "claim"), 1);
            assert_eq!(call_count(&pools, "adopt"), 1);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn native_create_retry_publishes_due_time_from_typed_observation_result() {
    for (error, delay) in [
        (pending_deadline(), 1),
        (
            AdapterError::NativeClaimNotReady(NativeClaimGuardReason::TimedOut),
            30,
        ),
        (AdapterError::Unavailable("transport".into()), 30),
    ] {
        let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let created = h.create().await;
        let pools = pools(&h, &[]);
        pools.pending_claim.store(true, Ordering::SeqCst);
        *pools.observe_error.lock().unwrap() = Some(error);
        let now = start_time(&created);
        cold_worker(&h, pools.clone(), true)
            .run_one(&created.id, now)
            .await
            .unwrap();
        let stored = h.stored(&created.id).await;
        assert_eq!(
            stored.work.ready_at,
            rfc3339_at(now + Duration::from_secs(delay))
        );
        let home = stored.lifecycle.home.as_deref().unwrap();
        let native = home.native_pool.as_ref().unwrap();
        assert_eq!(native.claim.as_ref(), pools.claim.lock().unwrap().as_ref());
        assert!(native.cleanup.is_none() && !native.cleaned && !home.released);
        assert_eq!(call_count(&pools, "claim"), 1);
        assert_eq!(call_count(&pools, "adopt"), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn native_create_retry_home_observation_outcomes_publish_due_time_without_changing_authority()
{
    for (error, delay, from_observation) in [
        (pending_deadline(), 1, true),
        (
            AdapterError::NativeClaimNotReady(NativeClaimGuardReason::TimedOut),
            30,
            false,
        ),
        (AdapterError::Unavailable("transport".into()), 30, false),
    ] {
        let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let created = h.create().await;
        let pools = pools(&h, &[]);
        *pools.adopt_error.lock().unwrap() = Some(error);
        let now = start_time(&created);
        let observing = tokio::time::Instant::now();
        cold_worker(&h, pools.clone(), true)
            .run_one(&created.id, now)
            .await
            .unwrap();
        let observed = observing.elapsed();
        let stored = h.stored(&created.id).await;
        let due = stamp_time(&stored.work.ready_at);
        if from_observation {
            // The shortened native delay starts when the observation completes,
            // rounded up to whole seconds. The paused clock can auto-advance
            // while the worker waits, so bound it by this test's own clock.
            let earliest = now + Duration::from_secs(delay);
            let latest = whole_seconds_up(now + observed) + Duration::from_secs(delay);
            assert!(
                due >= earliest && due <= latest,
                "due {} outside [{}, {}] after {observed:?}",
                stored.work.ready_at,
                rfc3339_at(earliest),
                rfc3339_at(latest)
            );
        } else {
            assert_eq!(
                stored.work.ready_at,
                rfc3339_at(now + Duration::from_secs(delay))
            );
        }
        let home = stored.lifecycle.home.as_deref().unwrap();
        let native = home.native_pool.as_ref().unwrap();
        let original = native.claim.as_ref().unwrap();
        assert_eq!(Some(original), pools.claim.lock().unwrap().as_ref());
        assert_eq!(original.sandbox_name.as_deref(), Some("warm-member"));
        assert!(native.cleanup.is_none() && !native.cleaned && !home.released);
        assert_eq!(call_count(&pools, "claim"), 1);
        assert_eq!(call_count(&pools, "adopt"), 0);
        assert_eq!(call_count(&pools, "reclaim"), 0);
        cold_worker(&h, pools.clone(), true)
            .run_one(&created.id, due)
            .await
            .unwrap();
        let recovered = h.stored(&created.id).await;
        let authority = recovered
            .lifecycle
            .home
            .as_ref()
            .unwrap()
            .receipt
            .as_ref()
            .unwrap()
            .native_authority
            .as_ref()
            .unwrap();
        assert_eq!(authority.claim, *original);
        assert_eq!(authority.sandbox_uid, "member-uid");
        assert_eq!(authority.pvc_uid, "pvc-uid");
        assert_eq!(call_count(&pools, "claim"), 1);
        assert_eq!(call_count(&pools, "adopt"), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn native_create_retry_delayed_assignment_keeps_deadline_provenance() {
    for (read_ms, due_seconds) in [(100, 6), (400, 30)] {
        let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let created = h.create().await;
        let pools = pools(&h, &[]);
        pools.pending_claim.store(true, Ordering::SeqCst);
        pools.observation.set_delay(Duration::from_millis(read_ms));
        let worker = cold_worker(&h, pools.clone(), true);
        let now = start_time(&created);
        let started = Instant::now();
        assert!(worker.run_one(&created.id, now).await.unwrap());
        assert_eq!(started.elapsed(), Duration::from_secs(5));
        let stored = h.stored(&created.id).await;
        // 100 ms responses finish before the last interval's deadline. A 400 ms
        // response is still in flight at expiry and must use ordinary scheduling.
        assert_eq!(
            stored.work.ready_at,
            rfc3339_at(now + Duration::from_secs(due_seconds))
        );
        assert_eq!(stored.work.attempts, 1);
        let home = stored.lifecycle.home.as_deref().unwrap();
        let native = home.native_pool.as_ref().unwrap();
        assert_eq!(native.claim.as_ref(), pools.claim.lock().unwrap().as_ref());
        assert!(home.receipt.is_none() && !home.released && native.cleanup.is_none());
        assert_eq!(call_count(&pools, "claim"), 1);
        assert_eq!(call_count(&pools, "adopt"), 0);
        assert_eq!(call_count(&pools, "reclaim"), 0);
    }
}

fn whole_seconds_up(at: SystemTime) -> SystemTime {
    let elapsed = at.duration_since(SystemTime::UNIX_EPOCH).unwrap();
    SystemTime::UNIX_EPOCH
        + Duration::from_secs(elapsed.as_secs() + u64::from(elapsed.subsec_nanos() > 0))
}
