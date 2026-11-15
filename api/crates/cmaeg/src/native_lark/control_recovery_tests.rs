use super::*;
use crate::{
    channels::resolver::ChannelResolver,
    ingress::{launch::UnavailableLauncher, routing_plan::fixture::keys},
    interactions::InteractionStore,
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
};
use axum::{http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn runtime() -> tokio::runtime::Runtime {
    // The test harness uses a larger stack; the production worker must not rely
    // on that inherited test setting, including in unoptimized builds.
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_stack_size(2 * 1024 * 1024)
        .enable_all()
        .build()
        .unwrap()
}

fn state(base: &str, interactions: bool) -> IngressState {
    let records = RecordClient::new(base, "record-fixture").unwrap();
    let keys = Arc::new(keys());
    let resolver =
        ChannelResolver::new(Arc::new(records.clone()), keys.clone(), Default::default());
    let state = IngressState::new(
        resolver,
        keys.clone(),
        Default::default(),
        Arc::new(UnavailableLauncher),
    )
    .unwrap();
    if !interactions {
        return state;
    }
    let providers = ProviderClient::new(
        ProviderRegistry::parse("[]").unwrap(),
        Duration::from_secs(1),
        Duration::from_secs(1),
        1,
    )
    .unwrap();
    state.with_interactions(Arc::new(InteractionStore::new(
        records,
        keys,
        Arc::new(providers),
    )))
}

async fn startup_and_drain(first_status: StatusCode) {
    let (sent, mut received) = tokio::sync::mpsc::unbounded_channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/api/v1/gateway/interaction-records",
        get({
            let calls = calls.clone();
            move || {
                let sent = sent.clone();
                let call = calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    sent.send(call).unwrap();
                    if call == 0 && first_status != StatusCode::OK {
                        first_status.into_response()
                    } else {
                        Json(serde_json::json!({"items":[]})).into_response()
                    }
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let readiness = Readiness::initialized();
    let mut worker = Worker::start(state(&base, true), readiness.clone());
    tokio::time::timeout(Duration::from_secs(5), async {
        // A second request proves the worker processed the first response and
        // survived its next poll, including the unavailable-store retry path.
        assert_eq!(received.recv().await, Some(0));
        assert_eq!(received.recv().await, Some(1));
    })
    .await
    .unwrap();
    assert!(worker.drain(Instant::now() + Duration::from_secs(2)).await);
    assert!(!readiness
        .report()
        .reasons
        .contains(&"routed_recovery_failed"));
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[test]
fn empty_recovery_starts_and_drains_on_default_worker_stack() {
    runtime().block_on(startup_and_drain(StatusCode::OK));
}

#[test]
fn unavailable_store_retries_and_drains_on_default_worker_stack() {
    runtime().block_on(startup_and_drain(StatusCode::SERVICE_UNAVAILABLE));
}

#[test]
fn unexpected_worker_exit_still_marks_readiness_failed() {
    runtime().block_on(async {
        let readiness = Readiness::initialized();
        let mut worker = Worker::start(state("http://127.0.0.1:1", false), readiness.clone());
        worker.task.take().unwrap().await.unwrap();
        assert!(readiness
            .report()
            .reasons
            .contains(&"routed_recovery_failed"));
    });
}
