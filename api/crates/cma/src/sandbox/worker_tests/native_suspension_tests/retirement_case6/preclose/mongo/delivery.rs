use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn recovered_dispatch_resumes_graceful_stop_through_capture_outage() {
    super::super::faults::recovered_dispatch_resumes_graceful_stop().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn retirement_capture_delivery_is_bounded_and_survives_authority_cancellation() {
    use crate::sandbox::native_retirement_delivery::CaptureDelivery;

    let f = Fixture::new().await;
    f.store
        .offer_retirement_capture(&f.parent, &f.child.id)
        .await
        .unwrap();
    let store = Arc::new(f.store.clone());
    let full = CaptureDelivery::new(store.clone(), 1);
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    assert!(full.tasks.spawn(async move {
        let _ = wait.await;
    }));
    assert!(matches!(
        f.authority
            .run(full.bind(&f.parent, &f.child))
            .await
            .unwrap(),
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(phase(&f.parent_row().await), "offered");
    release.send(()).unwrap();
    full.tasks.drain(Duration::from_secs(1)).await.unwrap();

    let delivery = CaptureDelivery::new(store, 1);
    let (transport, exchanges, server) = receiver().await;
    let runtime = RuntimeHandle::new(&f.parent.runtime.as_ref().unwrap().runtime_id);
    let result = f
        .authority
        .run(async {
            let (bound, mut pending) = delivery.bind(&f.parent, &f.child).await.unwrap();
            let request = RequestFrame::RuntimeStatus(layer3_adapter::RuntimeStatusRequest {
                sandbox_id: f.parent.id.to_string(),
                generation: 1,
                home_identity: "fixture".into(),
            });
            let _ = pending
                .scope(bound, transport.exchange(&runtime, request))
                .await;
            std::future::pending::<()>().await;
        })
        .await;
    assert!(result.is_err());
    delivery.tasks.drain(Duration::from_secs(5)).await.unwrap();
    assert_eq!(&*exchanges.lock().unwrap(), &["challenge"]);
    assert_eq!(phase(&f.parent_row().await), "observed");
    server.abort();
    f.cleanup().await;
}
