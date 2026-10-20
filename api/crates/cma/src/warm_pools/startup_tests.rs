use super::super::startup::{await_ready, observed_ready};
use super::*;

fn fixture() -> (Memory, Provider) {
    let calls = Arc::new(Mutex::new(vec![]));
    let original = target();
    let provider = Provider {
        calls: calls.clone(),
        original: original.plan.clone(),
        refuse_replaced_uid: AtomicBool::new(false),
        retired: AtomicBool::new(false),
    };
    let store = Memory {
        target: Mutex::new(original),
        bind_response_loss: AtomicBool::new(false),
        retirement_block: Mutex::new(std::collections::VecDeque::new()),
        drain_block: Mutex::new(std::collections::VecDeque::new()),
        calls,
    };
    (store, provider)
}

#[tokio::test(start_paused = true)]
async fn startup_adopts_prewarmed_binding_without_zeroing_capacity_before_readiness() {
    let (store, provider) = fixture();
    await_ready(
        &store,
        &provider,
        &provider.original,
        Duration::from_secs(120),
    )
    .await
    .unwrap();
    assert_eq!(
        *store.calls.lock().unwrap(),
        [
            "claim:wp_test",
            "inventory",
            "persist-binding",
            "resize:3",
            "finish:true",
            "inventory",
        ]
    );
    let target = store.target.lock().unwrap().clone();
    assert_eq!(target.binding, Some(provider.binding()));
    assert_eq!(target.applied_revision, target.revision);
    store.calls.lock().unwrap().clear();
    await_ready(
        &store,
        &provider,
        &provider.original,
        Duration::from_secs(120),
    )
    .await
    .unwrap();
    assert_eq!(*store.calls.lock().unwrap(), ["inventory"]);
}

#[tokio::test(start_paused = true)]
async fn startup_respects_durable_zero_target_without_provider_mutation_or_inventory() {
    let (store, provider) = fixture();
    store.target.lock().unwrap().target_replicas = 0;
    assert_eq!(store.target.lock().unwrap().requested.target_replicas, 3);
    await_ready(
        &store,
        &provider,
        &provider.original,
        Duration::from_secs(120),
    )
    .await
    .unwrap();
    assert!(store.calls.lock().unwrap().is_empty());
    assert!(store.target.lock().unwrap().binding.is_none());
}

#[tokio::test(start_paused = true)]
async fn startup_fails_within_deadline_when_target_cannot_be_warmed() {
    let (store, provider) = fixture();
    provider.refuse_replaced_uid.store(true, Ordering::SeqCst);
    let began = tokio::time::Instant::now();
    assert!(matches!(
        await_ready(
            &store,
            &provider,
            &provider.original,
            Duration::from_secs(120)
        )
        .await,
        Err(WarmPoolError::Unavailable),
    ));
    assert_eq!(began.elapsed(), Duration::from_secs(120));
    assert_eq!(store.target.lock().unwrap().applied_revision, 0);
    assert_eq!(store.target.lock().unwrap().target_replicas, 3);
}

#[test]
fn startup_accepts_one_spare_only_for_current_observed_durable_binding() {
    let (_, provider) = fixture();
    let mut target = target();
    target.binding = Some(provider.binding());
    target.applied_revision = target.revision;
    let ready = NativePoolStatus {
        binding: provider.binding(),
        desired_replicas: 3,
        ready_replicas: 1,
        observed: true,
    };
    assert!(observed_ready(&target, std::slice::from_ref(&ready)));
    for changed in 0..5 {
        let mut status = ready.clone();
        match changed {
            0 => status.binding.uid = "replacement".into(),
            1 => status.binding.template_uid = "replacement".into(),
            2 => status.desired_replicas = 2,
            3 => status.ready_replicas = 0,
            _ => status.observed = false,
        }
        assert!(!observed_ready(&target, &[status]));
    }
    target.applied_revision = 0;
    assert!(!observed_ready(&target, std::slice::from_ref(&ready)));
    target.applied_revision = target.revision;
    target.error = Some("unavailable".into());
    assert!(!observed_ready(&target, &[ready]));
}
