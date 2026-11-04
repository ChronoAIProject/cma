use super::*;
use crate::worker_credentials::{
    tests::{keys, Memory},
    WorkerCredentials, WorkerHistory,
};
use axum::{http::StatusCode, Router};
use cmaeg_core::{
    channel::Secret,
    ids::{BindingId, ProviderSlug},
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

#[tokio::test]
async fn registration_is_durable_before_provider_handoff_and_survives_failed_provider_response() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let app = Router::new().fallback(move || {
        let calls = observed.clone();
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let entries = serde_json::json!([{"slug":"external","base_url":origin,"kind":"responses_http","max_inflight":2,
        "paths":{"agent_card":"/card","binding":"/bindings/{binding_id}","conversation":"/bindings/{binding_id}/conversations/{conversation_id}","responses":"/responses"}}]);
    let history = Arc::new(Memory::default());
    history.hold.store(true, Ordering::SeqCst);
    let provider = Arc::new(
        ProviderClient::new(
            ProviderRegistry::parse(&entries.to_string()).unwrap(),
            Duration::from_secs(1),
            Duration::from_secs(1),
            2,
        )
        .unwrap()
        .with_worker_credentials(Some(WorkerCredentials::new(history.clone(), keys()))),
    );
    let profile =
        crate::channels::routing_fixture::definition(crate::channels::routing_fixture::routed())
            .profile;
    let worker = Secret::new(format!("nyx_{}", uuid::Uuid::new_v4()));
    let digest = crate::channels::definition::hash_secret(worker.as_str());
    let request = BindingRequest {
        schema_version: 1,
        target: Default::default(),
        profile_digest: profile.digest(),
        profile,
        agent_key: worker,
        tools: ToolsAdvertisement {
            native_mcp_url: "https://gateway.example/mcp".into(),
            nyxid_service_slug: "cmaeg".into(),
        },
    };
    let client = provider.clone();
    let pending = tokio::spawn(async move {
        client
            .bind(
                &ProviderSlug::new("external").unwrap(),
                &BindingId::new("binding_one").unwrap(),
                &request,
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), history.started.notified())
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!history.was_worker(&digest).await.unwrap());
    history.release.notify_one();
    assert!(matches!(
        pending.await.unwrap(),
        Err(ProviderError::Unavailable(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(history.was_worker(&digest).await.unwrap());
    server.abort();
}

#[tokio::test]
async fn failed_registration_blocks_credentials_before_any_transport() {
    let history = Arc::new(Memory::default());
    history.fail.store(true, Ordering::SeqCst);
    let client = ProviderClient::new(
        ProviderRegistry::parse("[]").unwrap(),
        Duration::from_secs(1),
        Duration::from_secs(1),
        1,
    )
    .unwrap()
    .with_worker_credentials(Some(WorkerCredentials::new(history.clone(), keys())));
    let result = client
        .unbind(
            &ProviderSlug::new("external").unwrap(),
            &BindingId::new("binding_one").unwrap(),
            &Secret::new("worker-fixture"),
        )
        .await;
    assert!(matches!(result, Err(ProviderError::Unavailable(_))));
    assert_eq!(history.calls.load(Ordering::SeqCst), 1);
    assert!(history.known.lock().unwrap().is_empty());
}
