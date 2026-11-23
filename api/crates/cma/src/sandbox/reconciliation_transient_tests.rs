use super::super::provider_error;
use super::classify;
use futures_util::{stream::FuturesUnordered, StreamExt};
use layer2_core::sandbox::SandboxStoreError;
use layer3_adapter::AdapterError;

fn stream_timeout() -> AdapterError {
    AdapterError::Unavailable("Kubernetes runtime stream opening timed out".into())
}

fn store_unavailable() -> SandboxStoreError {
    SandboxStoreError::Unavailable("sandbox store write failed".into())
}

#[tokio::test]
async fn only_the_attempts_own_transient_runtime_failure_is_transient() {
    for message in [
        "Kubernetes runtime stream opening timed out",
        "runtime assignment timed out",
    ] {
        let (result, transient) = classify(async {
            Err::<(), _>(provider_error(AdapterError::Unavailable(message.into())))
        })
        .await;
        assert!(transient, "{message}");
        assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
    }
    let (_, transient) = classify(async { Err::<(), _>(store_unavailable()) }).await;
    assert!(!transient, "a store failure stays fatal");
    for refusal in [
        AdapterError::Rejected("runtime refused adoption".into()),
        AdapterError::InputAuthorityUnavailable("input authority".into()),
        AdapterError::AtCapacity("no slot".into()),
    ] {
        let (_, transient) = classify(async { Err::<(), _>(provider_error(refusal)) }).await;
        assert!(!transient, "non-transient runtime refusals stay fatal");
    }
    let (result, transient) = classify(async { Ok::<bool, SandboxStoreError>(true) }).await;
    assert!(!transient);
    assert!(matches!(result, Ok(true)));
}

#[tokio::test]
async fn a_swallowed_runtime_failure_never_reclassifies_a_later_store_failure() {
    let (_, transient) = classify(async {
        let _swallowed = provider_error(stream_timeout());
        Err::<(), _>(store_unavailable())
    })
    .await;
    assert!(!transient);
    let (_, transient) = classify(async {
        let _swallowed = provider_error(stream_timeout());
        Err::<(), _>(provider_error(AdapterError::Rejected("refused".into())))
    })
    .await;
    assert!(!transient);
}

#[tokio::test]
async fn mapping_outside_an_attempt_or_inside_a_spawned_task_fails_closed() {
    // Outside an attempt nothing is recorded and the mapping is unchanged.
    let mapped = provider_error(stream_timeout());
    assert!(matches!(
        &mapped,
        SandboxStoreError::Unavailable(message)
            if message == "sandbox runtime authority is unavailable: runtime unavailable: Kubernetes runtime stream opening timed out"
    ));
    // Task-locals do not reach spawned tasks, so that path stays fatal.
    let (_, transient) = classify(async {
        let mapped = tokio::spawn(async { provider_error(stream_timeout()) })
            .await
            .unwrap();
        Err::<(), _>(mapped)
    })
    .await;
    assert!(!transient);
}

#[tokio::test]
async fn concurrent_attempts_do_not_share_their_classification() {
    let mut attempts = FuturesUnordered::new();
    attempts.push(Box::pin(classify(async {
        let mapped = provider_error(stream_timeout());
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        Err::<&str, _>(mapped)
    }))
        as std::pin::Pin<Box<dyn std::future::Future<Output = _>>>);
    attempts.push(Box::pin(classify(async {
        tokio::task::yield_now().await;
        // Same text as the other attempt's mapped error, but never recorded here.
        Err::<&str, _>(SandboxStoreError::Unavailable(
            "sandbox runtime authority is unavailable: runtime unavailable: Kubernetes runtime stream opening timed out".into(),
        ))
    })));
    let mut transient = Vec::new();
    while let Some((_, value)) = attempts.next().await {
        transient.push(value);
    }
    transient.sort();
    assert_eq!(transient, vec![false, true]);
}
