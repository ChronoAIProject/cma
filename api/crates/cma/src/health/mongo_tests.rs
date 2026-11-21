use std::sync::atomic::Ordering;

use super::{
    mongo_fixture::{DriverProbe, MongoPeer},
    tests::response,
    *,
};

async fn wait_for_reason(app: &Router, reason: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let (status, body) = response(app, "/ready").await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            if body["dependency"]["reason"] == reason {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("readiness did not report the application deadline");
}

async fn wait_for_recovery(app: &Router) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while response(app, "/ready").await.0 != StatusCode::OK {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("readiness did not recover");
}

#[tokio::test]
async fn unanswered_mongo_ping_is_cancelled_closes_connection_and_recovers() {
    let peer = MongoPeer::start().await;
    peer.block(true);
    let app = router(Arc::new(DriverProbe(peer.client.clone())));
    peer.wait_for(|| peer.ping_count() == 2).await;
    assert_eq!(peer.in_use(), 1);
    for _ in 0..50 {
        assert_eq!(
            response(&app, "/ready").await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    assert_eq!(peer.ping_count(), 2);
    assert_eq!(response(&app, "/livez").await.0, StatusCode::OK);
    wait_for_reason(&app, "deadline").await;
    peer.wait_for(|| peer.in_use() == 0 && peer.peer_closed.load(Ordering::SeqCst) == 1)
        .await;
    assert_eq!(peer.trace.lock().unwrap().dropped, 1);
    peer.block(false);
    wait_for_recovery(&app).await;
    peer.wait_for(|| peer.in_use() == 0).await;
    drop(app);
    peer.client.clone().shutdown().await;
}

#[tokio::test]
async fn saturated_mongo_checkout_is_bounded_and_recovery_reuses_the_pool() {
    let peer = MongoPeer::start().await;
    peer.block(true);
    let client = peer.client.clone();
    let holder = tokio::spawn(async move { DriverProbe(client).ping().await });
    peer.wait_for(|| peer.ping_count() == 2).await;
    let app = router(Arc::new(DriverProbe(peer.client.clone())));
    peer.wait_for(|| peer.trace.lock().unwrap().checkout_started == 3)
        .await;
    wait_for_reason(&app, "deadline").await;
    assert_eq!(
        peer.ping_count(),
        2,
        "readiness must still be waiting for checkout"
    );
    assert_eq!(
        peer.in_use(),
        1,
        "only the unrelated holder owns a connection"
    );
    assert_eq!(response(&app, "/livez").await.0, StatusCode::OK);
    peer.block(false);
    tokio::time::timeout(Duration::from_secs(1), holder)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    wait_for_recovery(&app).await;
    peer.wait_for(|| peer.in_use() == 0).await;
    assert_eq!(peer.trace.lock().unwrap().dropped, 0);
    drop(app);
    peer.client.clone().shutdown().await;
}

#[tokio::test]
async fn router_shutdown_releases_an_inflight_mongo_connection() {
    let peer = MongoPeer::start().await;
    peer.block(true);
    let app = router(Arc::new(DriverProbe(peer.client.clone())));
    peer.wait_for(|| peer.ping_count() == 2).await;
    drop(app);
    peer.wait_for(|| peer.in_use() == 0 && peer.peer_closed.load(Ordering::SeqCst) == 1)
        .await;
    peer.block(false);
    tokio::time::timeout(
        Duration::from_secs(1),
        DriverProbe(peer.client.clone()).ping(),
    )
    .await
    .unwrap()
    .unwrap();
    peer.wait_for(|| peer.in_use() == 0).await;
    assert_eq!(peer.trace.lock().unwrap().dropped, 1);
    peer.client.clone().shutdown().await;
}

#[tokio::test]
async fn background_deadline_and_cancellation_release_real_driver_connections() {
    use crate::background::{BackgroundBudget, BackgroundWork};
    use layer2_core::sandbox::store::SandboxStoreError;

    let peer = MongoPeer::start().await;
    let budget = BackgroundBudget::new();
    peer.block(true);
    let work = BackgroundWork::new(
        budget.clone(),
        "deadline_test",
        Duration::from_millis(200),
        Duration::ZERO,
    );
    let client = peer.client.clone();
    let timed = tokio::spawn(async move {
        work.run(|| async {
            DriverProbe(client)
                .ping()
                .await
                .map_err(|_| SandboxStoreError::Unavailable("fixture".into()))
        })
        .await
    });
    peer.wait_for(|| peer.ping_count() == 2).await;
    assert!(timed.await.unwrap().is_err());
    peer.wait_for(|| peer.in_use() == 0 && peer.peer_closed.load(Ordering::SeqCst) == 1)
        .await;

    let work = BackgroundWork::new(
        budget.clone(),
        "cancel_test",
        Duration::from_secs(5),
        Duration::ZERO,
    );
    let client = peer.client.clone();
    let cancelled = tokio::spawn(async move {
        work.run(|| async {
            DriverProbe(client)
                .ping()
                .await
                .map_err(|_| SandboxStoreError::Unavailable("fixture".into()))
        })
        .await
    });
    peer.wait_for(|| peer.ping_count() == 3).await;
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    peer.wait_for(|| peer.in_use() == 0 && peer.peer_closed.load(Ordering::SeqCst) == 2)
        .await;
    assert_eq!(peer.trace.lock().unwrap().dropped, 2);
    peer.block(false);
    let work = BackgroundWork::new(
        budget,
        "recover_test",
        Duration::from_secs(1),
        Duration::ZERO,
    );
    assert_eq!(
        work.run(|| async {
            DriverProbe(peer.client.clone())
                .ping()
                .await
                .map_err(|_| SandboxStoreError::Unavailable("fixture".into()))
        })
        .await
        .unwrap(),
        Some(())
    );
    peer.client.clone().shutdown().await;
}
