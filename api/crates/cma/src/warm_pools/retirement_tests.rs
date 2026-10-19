use super::*;

fn fixture(blocks: &[bool]) -> (Memory, Provider) {
    let calls = Arc::new(Mutex::new(vec![]));
    let mut original = target();
    let provider = Provider {
        calls: calls.clone(),
        original: original.plan.clone(),
        refuse_replaced_uid: AtomicBool::new(false),
        retired: AtomicBool::new(false),
    };
    original.binding = Some(provider.binding());
    let store = Memory {
        target: Mutex::new(original),
        bind_response_loss: AtomicBool::new(false),
        retirement_block: Mutex::new(blocks.iter().copied().collect()),
        calls,
    };
    (store, provider)
}

async fn reconcile(store: &Memory, provider: &Provider) {
    let original = store.target.lock().unwrap().clone();
    super::super::retirement::retire_claimed(store, provider, "current", original)
        .await
        .unwrap();
}

#[tokio::test]
async fn late_allocation_after_provider_marker_prevents_drain_and_definition_removal() {
    let (store, provider) = fixture(&[false, true]);
    reconcile(&store, &provider).await;
    assert_eq!(
        *store.calls.lock().unwrap(),
        [
            "scan",
            "persist-retiring",
            "mark-provider",
            "scan",
            "finish:true"
        ]
    );
    assert_eq!(
        store.target.lock().unwrap().binding,
        Some(provider.binding())
    );
}

#[tokio::test]
async fn existing_user_prevents_admission_marker_or_any_provider_mutation() {
    let (store, provider) = fixture(&[true]);
    reconcile(&store, &provider).await;
    assert_eq!(*store.calls.lock().unwrap(), ["scan", "finish:true"]);
    assert!(!store.target.lock().unwrap().retiring);
}

#[tokio::test]
async fn retirement_drains_before_removal_and_replays_without_recreating_definitions() {
    let (store, provider) = fixture(&[]);
    reconcile(&store, &provider).await;
    assert_eq!(
        *store.calls.lock().unwrap(),
        [
            "scan",
            "persist-retiring",
            "mark-provider",
            "scan",
            "prove-retirement",
            "resize:0",
            "finish:true"
        ]
    );
    store.calls.lock().unwrap().clear();
    provider.retired.store(true, Ordering::SeqCst);
    reconcile(&store, &provider).await;
    assert_eq!(
        *store.calls.lock().unwrap(),
        [
            "scan",
            "persist-retiring",
            "mark-provider",
            "scan",
            "prove-retirement",
            "remove-target"
        ]
    );
}
