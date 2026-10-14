use super::*;
use tokio::time::Instant;

fn call_count(pools: &Pools, operation: &str) -> usize {
    pools
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|call| **call == operation)
        .count()
}

#[tokio::test(start_paused = true)]
async fn pending_assignment_provisions_in_one_attempt_without_another_allocation() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let pools = pools(&harness, &[]);
    pools.pending_claim.store(true, Ordering::SeqCst);
    let worker = worker(&harness, pools.clone());
    let started = Instant::now();
    let assign = async {
        tokio::time::sleep(Duration::from_millis(750)).await;
        pools.pending_claim.store(false, Ordering::SeqCst);
    };
    let (result, ()) = tokio::join!(worker.run_one(&created.id, start_time(&created)), assign);
    assert!(result.unwrap());
    let provisioned = harness.stored(&created.id).await;
    assert!(provisioned.runtime.is_some());
    assert_eq!(provisioned.work.attempts, 0);
    assert_eq!(provisioned.work.step, SandboxWorkStep::RunInstall);
    let home = provisioned.lifecycle.home.unwrap();
    let claim = home.native_pool.as_ref().unwrap().claim.as_ref().unwrap();
    assert_eq!(claim.uid, "claim-uid");
    assert_eq!(claim.sandbox_name.as_deref(), Some("warm-member"));
    assert!(home.receipt.is_some());
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(call_count(&pools, "select"), 1);
    assert_eq!(call_count(&pools, "claim"), 1);
    assert!(call_count(&pools, "observe") > 1);
    assert_eq!(call_count(&pools, "adopt"), 1);
}

#[tokio::test(start_paused = true)]
async fn sustained_pending_assignment_is_bounded_and_retries_the_persisted_claim() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let pools = pools(&harness, &[]);
    pools.pending_claim.store(true, Ordering::SeqCst);
    let worker = worker(&harness, pools.clone());
    let now = start_time(&created);
    let started = Instant::now();
    assert!(worker.run_one(&created.id, now).await.unwrap());
    assert_eq!(started.elapsed(), Duration::from_secs(5));
    let pending = harness.stored(&created.id).await;
    assert!(pending.runtime.is_none());
    assert_eq!(pending.work.attempts, 1);
    assert_eq!(pending.work.step, SandboxWorkStep::ProvisionRuntime);
    let home = pending.lifecycle.home.as_ref().unwrap();
    let original = home.native_pool.as_ref().unwrap().claim.clone().unwrap();
    assert!(original.sandbox_name.is_none());
    assert!(home.receipt.is_none());
    assert_eq!(call_count(&pools, "claim"), 1);
    assert_eq!(call_count(&pools, "adopt"), 0);
    let observed = call_count(&pools, "observe");
    tokio::time::advance(Duration::from_secs(10)).await;
    assert_eq!(call_count(&pools, "observe"), observed);
    assert!(!worker
        .run_one(&created.id, now + Duration::from_secs(6))
        .await
        .unwrap());

    pools.pending_claim.store(false, Ordering::SeqCst);
    assert!(worker
        .run_one(&created.id, now + Duration::from_secs(36))
        .await
        .unwrap());
    let recovered = harness.stored(&created.id).await;
    assert!(recovered.runtime.is_some());
    let assigned = pools.claim.lock().unwrap().clone().unwrap();
    assert_eq!(assigned.uid, original.uid);
    assert_eq!(assigned.request, original.request);
    assert_eq!(call_count(&pools, "select"), 1);
    assert_eq!(call_count(&pools, "claim"), 1);
    assert_eq!(call_count(&pools, "adopt"), 1);
}

#[tokio::test(start_paused = true)]
async fn assignment_dependency_and_identity_errors_return_without_polling_or_adoption() {
    for error in [
        AdapterError::Unavailable("fixture transport failure".into()),
        AdapterError::Rejected("native claim UID changed".into()),
    ] {
        let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let created = harness.create().await;
        let pools = pools(&harness, &["observe"]);
        worker(&harness, pools.clone())
            .run_one(&created.id, start_time(&created))
            .await
            .unwrap();
        let persisted = harness.stored(&created.id).await.lifecycle.home.unwrap();
        let expected = error.to_string();
        *pools.observe_error.lock().unwrap() = Some(error);
        let adapter = CodexAdapter::with_backend(
            Backend {
                pools: pools.clone(),
                backend: harness.backend.clone(),
            },
            CodexSandboxSpec::default(),
        )
        .with_home_volumes(pools.clone());
        let started = Instant::now();
        let error = crate::sandbox::home_native::next(&adapter, &persisted)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert_eq!(started.elapsed(), Duration::ZERO);
        assert_eq!(call_count(&pools, "observe"), 2);
        assert_eq!(call_count(&pools, "claim"), 1);
        assert_eq!(call_count(&pools, "adopt"), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn cancelling_assignment_wait_retains_original_claim_without_detached_work() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let pools = pools(&harness, &[]);
    pools.pending_claim.store(true, Ordering::SeqCst);
    let worker = worker(&harness, pools.clone());
    let id = created.id.clone();
    let now = start_time(&created);
    let task = tokio::spawn(async move { worker.run_one(&id, now).await });
    pools.observed.notified().await;
    assert!(!task.is_finished());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let original = harness.stored(&created.id).await.lifecycle.home.unwrap();
    let claim = original
        .native_pool
        .as_ref()
        .unwrap()
        .claim
        .clone()
        .unwrap();
    assert_eq!(claim.uid, "claim-uid");
    assert!(claim.sandbox_name.is_none());
    assert!(original.receipt.is_none());
    let observed = call_count(&pools, "observe");
    pools.pending_claim.store(false, Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(10)).await;
    assert_eq!(call_count(&pools, "observe"), observed);
    assert_eq!(pools.claim.lock().unwrap().as_ref(), Some(&claim));
    assert_eq!(call_count(&pools, "claim"), 1);
    assert_eq!(call_count(&pools, "adopt"), 0);
}
