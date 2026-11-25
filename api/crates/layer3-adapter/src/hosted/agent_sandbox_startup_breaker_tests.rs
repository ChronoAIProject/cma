use super::*;
use std::sync::{Arc, Barrier};

fn identity(n: u32) -> StartupIncarnation {
    StartupIncarnation {
        boot: "boot-one".into(),
        process: format!("process-{n}"),
        restart: n,
    }
}

fn delivered(breaker: &StartupBreaker, handle: &RuntimeHandle, n: u32) {
    breaker.delivered(
        handle,
        identity(n),
        "sbx_one".into(),
        7,
        SystemTime::UNIX_EPOCH + Duration::from_secs(100),
    );
}

fn fail(breaker: &StartupBreaker, handle: &RuntimeHandle, n: u32, seconds: u64, failed: bool) {
    let pending = breaker.evidence_needed(handle, &identity(n + 1)).unwrap();
    let evidence = StartupTermination {
        process: identity(n).process,
        failed,
        class: RuntimeStartupFailureClass::ExitedBeforeServing,
        finished_at: SystemTime::UNIX_EPOCH + Duration::from_secs(100 + seconds),
    };
    breaker.observe(handle, &pending, Some(evidence.clone()));
    breaker.observe(handle, &pending, Some(evidence));
}

#[tokio::test(start_paused = true)]
async fn three_failed_incarnations_open_and_concurrent_admission_claims_one_probe() {
    let breaker = Arc::new(StartupBreaker::default());
    let handle = RuntimeHandle("test-runtime".into());
    for n in 0..3 {
        assert!(breaker.admit(&handle).is_ok());
        delivered(&breaker, &handle, n);
        fail(&breaker, &handle, n, 60, true);
        assert_eq!(breaker.failure(&handle).is_some(), n == 2);
    }
    assert_eq!(breaker.failure(&handle).unwrap().consecutive, 3);
    for minutes in [15, 30, 60, 60] {
        let expected = RuntimeStartupProbe {
            runtime: handle.clone(),
            sandbox_id: "sbx_one".into(),
            generation: 7,
            next_probe_at: Instant::now() + Duration::from_secs(minutes * 60),
        };
        tokio::time::advance(Duration::from_secs(minutes * 60 - 1)).await;
        for error in [
            breaker.hold_before_wait(&handle).unwrap(),
            breaker.admit(&handle).unwrap_err(),
        ] {
            let AdapterError::RuntimeStartupHold(failure) = error else {
                panic!("the closed breaker must report a startup hold");
            };
            assert_eq!(failure.probe(), Some(&expected));
        }
        assert_eq!(breaker.failure(&handle).unwrap().probe(), Some(&expected));
        tokio::time::advance(Duration::from_secs(1)).await;
        let mut tasks = Vec::new();
        for _ in 0..20 {
            let breaker = breaker.clone();
            let handle = handle.clone();
            tasks.push(tokio::spawn(async move { breaker.admit(&handle) }));
        }
        let mut admitted = 0;
        for task in tasks {
            match task.await.unwrap() {
                Ok(()) => admitted += 1,
                Err(AdapterError::RuntimeStartupHold(failure)) => {
                    assert_eq!(failure.consecutive, 3);
                }
                other => panic!("unexpected admission: {other:?}"),
            }
        }
        assert_eq!(admitted, 1);
    }
}

#[tokio::test(start_paused = true)]
async fn nonfast_missing_and_wrong_process_evidence_reset_the_sequence() {
    let handle = RuntimeHandle("test-runtime".into());
    for kind in ["slow", "success", "missing", "different"] {
        let breaker = StartupBreaker::default();
        for n in 0..3 {
            delivered(&breaker, &handle, n);
            fail(&breaker, &handle, n, 1, true);
        }
        assert!(breaker.failure(&handle).is_some());
        delivered(&breaker, &handle, 3);
        match kind {
            "slow" => fail(&breaker, &handle, 3, 61, true),
            "success" => fail(&breaker, &handle, 3, 1, false),
            _ => {
                let pending = breaker.evidence_needed(&handle, &identity(4)).unwrap();
                let evidence = (kind == "different").then(|| StartupTermination {
                    process: "different".into(),
                    failed: true,
                    class: RuntimeStartupFailureClass::Terminated,
                    finished_at: SystemTime::UNIX_EPOCH,
                });
                breaker.observe(&handle, &pending, evidence);
            }
        }
        assert!(breaker.failure(&handle).is_none());
        assert!(breaker.admit(&handle).is_ok());
        delivered(&breaker, &handle, 4);
        fail(&breaker, &handle, 4, 1, true);
        assert!(breaker.failure(&handle).is_none());
    }
}

#[tokio::test]
async fn boot_reset_forgetting_and_eviction_keep_state_bounded() {
    let breaker = StartupBreaker::default();
    let handle = RuntimeHandle("test-runtime".into());
    for reset in [false, true] {
        for n in 0..3 {
            delivered(&breaker, &handle, n);
            fail(&breaker, &handle, n, 1, true);
        }
        if reset {
            breaker.reset(&handle);
        } else {
            breaker.reset_boot(&handle, "new-boot");
        }
        assert!(breaker.admit(&handle).is_ok());
        assert!(breaker.failure(&handle).is_none());
    }
    for n in 0..1100 {
        delivered(&breaker, &RuntimeHandle(format!("runtime-{n}")), 0);
    }
    assert_eq!(breaker.entries.lock().unwrap().len(), MAX_RECORDS);
}

#[test]
fn concurrent_observations_and_late_duplicate_acknowledgements_count_one_failed_incarnation() {
    let breaker = StartupBreaker::default();
    let handle = RuntimeHandle("test-runtime".into());
    delivered(&breaker, &handle, 0);
    let first = breaker.evidence_needed(&handle, &identity(1)).unwrap();
    let second = breaker.evidence_needed(&handle, &identity(1)).unwrap();
    let barrier = Barrier::new(2);
    std::thread::scope(|scope| {
        for pending in [first, second] {
            let breaker = &breaker;
            let handle = &handle;
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                breaker.observe(
                    handle,
                    &pending,
                    Some(StartupTermination {
                        process: identity(0).process,
                        failed: true,
                        class: RuntimeStartupFailureClass::ExitedBeforeServing,
                        finished_at: SystemTime::UNIX_EPOCH + Duration::from_secs(102),
                    }),
                );
            });
        }
    });
    assert_eq!(
        breaker.entries.lock().unwrap()[&handle].failure.consecutive,
        1
    );
    breaker.delivered(
        &handle,
        identity(0),
        "sbx_one".into(),
        7,
        SystemTime::UNIX_EPOCH + Duration::from_secs(103),
    );
    assert!(breaker.evidence_needed(&handle, &identity(1)).is_none());
    assert_eq!(
        breaker.entries.lock().unwrap()[&handle].failure.consecutive,
        1
    );
    delivered(&breaker, &handle, 1);
    let pending = breaker.evidence_needed(&handle, &identity(2)).unwrap();
    delivered(&breaker, &handle, 2);
    delivered(&breaker, &handle, 1);
    breaker.observe(&handle, &pending, None);
    let entries = breaker.entries.lock().unwrap();
    assert_eq!(
        entries[&handle].last_delivery.as_ref().unwrap().incarnation,
        identity(2)
    );
    assert_eq!(entries[&handle].failure.consecutive, 1);
}
