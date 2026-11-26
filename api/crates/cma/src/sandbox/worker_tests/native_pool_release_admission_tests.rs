use super::*;
use crate::warm_pools::tests::runtime_pointer::released_source::{Call, ControlledSource};
use layer3_adapter::ReleasedRuntime;

fn release(revision: u64, digest: char) -> ReleasedRuntime {
    ReleasedRuntime {
        lane_revisions: None,
        revision,
        agent: format!(
            "registry.example/runtime@sha256:{}",
            digest.to_string().repeat(64)
        ),
        machine: None,
    }
}

fn recording_pools(harness: &Harness, failures: &[&'static str]) -> Arc<Pools> {
    let mut pools = pools(harness, failures);
    Arc::get_mut(&mut pools).unwrap().released_selections = Some(Mutex::new(vec![]));
    pools
}

fn released_worker(
    harness: &Harness,
    holder: &str,
    pools: Arc<Pools>,
    source: Arc<ControlledSource>,
) -> Arc<SandboxWorker> {
    let config = SandboxWorkerConfig::new(holder, "test", runtime_image(), harness.policy.clone())
        .with_released_runtime(source);
    worker_with_config(harness, pools, config)
}

fn assert_pin(sandbox: &Sandbox, expected: &ReleasedRuntime) {
    let home = sandbox.lifecycle.home.as_ref().unwrap();
    let claim = home.native_pool.as_ref().unwrap();
    assert_eq!(claim.request.pool.spec.image.uri, expected.agent);
    assert_eq!(claim.claim.as_ref().unwrap().request, claim.request);
    assert_eq!(
        sandbox.runtime.as_ref().unwrap().runtime_version,
        expected.agent
    );
}

#[tokio::test]
async fn managed_provisioning_keeps_the_image_admitted_at_home_reservation() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let original = harness.create().await;
    let first = release(1, 'a');
    let current = release(2, 'b');
    let source = Arc::new(ControlledSource::new(first.clone()));
    source.advance_after_validation(current.clone());
    let pools = recording_pools(&harness, &["claim"]);
    let now = start_time(&original);
    let worker = released_worker(&harness, "worker-a", pools.clone(), source.clone());
    assert!(worker.run_one(&original.id, now).await.unwrap());
    let reserved = harness.stored(&original.id).await;
    let reservation = reserved
        .lifecycle
        .home
        .as_ref()
        .unwrap()
        .native_pool
        .as_ref()
        .unwrap();
    assert_eq!(reservation.request.pool.spec.image.uri, first.agent);
    assert!(reserved.runtime.is_none());
    assert_eq!(
        source.take_calls(),
        [Call::Snapshot(1), Call::RequireCurrent(1, true)]
    );

    // Recreate the worker after a lost Claim response and retain the admitted pin.
    drop(worker);
    let restarted = released_worker(&harness, "worker-b", pools.clone(), source.clone());
    assert!(restarted
        .run_one(&original.id, now + Duration::from_secs(31))
        .await
        .unwrap());
    let recovered = harness.stored(&original.id).await;
    assert_pin(&recovered, &first);
    assert_eq!(
        recovered
            .lifecycle
            .home
            .as_ref()
            .unwrap()
            .native_pool
            .as_ref()
            .unwrap()
            .request,
        reservation.request,
    );
    assert_eq!(source.take_calls(), [Call::Snapshot(2)]);
    assert_eq!(
        *pools.released_selections.as_ref().unwrap().lock().unwrap(),
        [first.agent]
    );

    let new = harness.create().await;
    let new_pools = recording_pools(&harness, &[]);
    let new_worker = released_worker(&harness, "worker-b", new_pools, source.clone());
    assert!(new_worker.run_one(&new.id, start_time(&new)).await.unwrap());
    assert_pin(&harness.stored(&new.id).await, &current);
    assert_eq!(
        source.take_calls(),
        [
            Call::Snapshot(2),
            Call::RequireCurrent(2, true),
            Call::Snapshot(2)
        ]
    );
}

#[tokio::test]
async fn missed_refresh_is_rejected_before_any_stale_allocation() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let first = release(1, 'a');
    let latest = release(2, 'b');
    let source = Arc::new(ControlledSource::new(first.clone()));
    source.miss_refresh(latest.clone());
    let pools = recording_pools(&harness, &[]);
    let worker = released_worker(&harness, "worker-a", pools.clone(), source.clone());
    let now = start_time(&created);

    assert!(worker.run_one(&created.id, now).await.unwrap());
    let refused = harness.stored(&created.id).await;
    assert!(refused.lifecycle.home.is_none());
    assert!(refused.runtime.is_none());
    assert!(pools.claim.lock().unwrap().is_none());
    assert_eq!(*pools.calls.lock().unwrap(), ["select"]);
    assert_eq!(
        source.take_calls(),
        [Call::Snapshot(1), Call::RequireCurrent(1, false)]
    );

    assert!(worker
        .run_one(&created.id, now + Duration::from_secs(31))
        .await
        .unwrap());
    assert_pin(&harness.stored(&created.id).await, &latest);
    assert_eq!(
        source.take_calls(),
        [
            Call::Snapshot(2),
            Call::RequireCurrent(2, true),
            Call::Snapshot(2)
        ]
    );
    assert_eq!(
        *pools.released_selections.as_ref().unwrap().lock().unwrap(),
        [first.agent, latest.agent],
    );
}
