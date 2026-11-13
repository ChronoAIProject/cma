use super::*;
use crate::turns::routed::{
    adapter_test_server::HttpServer,
    scheduler::{Disposition, TargetExecutor, WorkItem},
    test_support::{supported, Setup},
    HttpExecutionPort, RoutedDelivery, RoutedTargetExecutor,
};
use crate::{
    delivery::{durable::DeliveryCoordinator, router::ShardedScheduler},
    http::readiness::Readiness,
    turns::TurnTracker,
    DeliveryConfig,
};
use axum::{routing::get, Router};
use serde_json::{json, Value};
use tokio::sync::mpsc;

#[tokio::test]
async fn expired_non_admission_repairs_both_reasons_through_public_activity_api() {
    let (_records, mut state, mut request, mut creator) = fixture_app().await;
    let _auth = live_auth(&mut state, &mut creator).await;
    state.activity.received(&request).await.unwrap();
    let channel = request.channel.id.to_string();
    let channel_id = request.channel.id.clone();
    let partition = request
        .activity
        .partition_key(&request.channel.definition.partition);
    let conversation =
        cmaeg_core::ids::ConversationId::derive(&fixture::keys(), &channel_id, 1, &partition);
    request.conversation_id = conversation.clone();
    fixture::rewrite_ref(&mut request, |event| {
        event.channel_id = channel_id;
        event.conversation_id = conversation;
    });
    let setup = Setup::with_request(supported(), request).await;
    let http = HttpServer::start().await;
    let cases: Vec<Value> = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/provider_sender_refusals.json"
    )))
    .unwrap();
    {
        let mut memory = http.memory.lock().unwrap();
        memory.config.post_status = 503;
        memory.config.post_error_body = Some(cases[2]["body"].to_string());
        memory.config.post_error_retry_after = Some("5".into());
    }
    let work = WorkItem {
        identity: setup.plan.identity.clone(),
        target_id: setup.target().id.clone(),
    };
    // No activity observer in this first process: the last refusal must be
    // recovered from durable diagnostic evidence, not an in-memory notification.
    let first = Arc::new(HttpExecutionPort::new(
        http.client(true, Duration::from_secs(1)),
        setup.port.clone(),
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
    ))
    .dispatcher();
    for _ in 0..2 {
        assert!(matches!(
            first
                .dispatch(&work.identity, &work.target_id, "before-restart")
                .await
                .unwrap(),
            crate::turns::routed::DispatchOutcome::Retry
        ));
    }
    assert_eq!(
        http.seen()
            .iter()
            .filter(|request| request.method == "POST")
            .count(),
        2
    );
    setup
        .server
        .memory
        .clock
        .store(setup.target().launch.event.expires_at, Ordering::SeqCst);
    let journal = setup
        .journal()
        .clone()
        .with_activity(state.activity.clone());
    let port = Arc::new(HttpExecutionPort::new(
        http.client(true, Duration::from_secs(1)),
        setup.port.clone(),
        setup.server.store(),
        journal.clone(),
        setup.current.clone(),
    ));
    let (tx, mut rx) = mpsc::channel(1);
    let delivery = RoutedDelivery::new(
        setup.server.store(),
        journal,
        setup.current.clone(),
        DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(tx))),
    );
    let executor = RoutedTargetExecutor::new(
        port,
        delivery,
        TurnTracker::new(Readiness::initialized()),
        &DeliveryConfig::default(),
    );
    for _ in 0..2 {
        assert!(matches!(
            executor.execute(&work).await,
            Disposition::Settled
        ));
    }
    let api = Router::new()
        .route(
            "/v1/channels/{channel_id}/activity",
            get(crate::http::activity::latest),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/v1/channels/{channel}/activity",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move { axum::serve(listener, api).await.unwrap() });
    let response = reqwest::Client::new()
        .get(url)
        .header("authorization", "Bearer activity-fixture")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let page: Value = response.json().await.unwrap();
    server.abort();
    let consumer = &page["items"][0]["consumers"][0];
    assert_eq!(consumer["state"], "failed");
    assert_eq!(consumer["refusal_code"], "event_expired_before_start");
    assert_eq!(
        consumer["last_non_admission_code"],
        "verified_sender_context_unavailable"
    );
    assert!(consumer["completed_at"].is_string());
    assert_eq!(consumer["deliveries"], json!([]));
    assert!(consumer["response_id"].is_null());
    assert!(rx.try_recv().is_err());
    assert_eq!(
        http.seen()
            .iter()
            .filter(|request| request.method == "POST")
            .count(),
        2
    );
    let recovery = setup
        .server
        .store()
        .recover_all_page(None, 32)
        .await
        .unwrap();
    assert!(
        recovery.issues.is_empty(),
        "diagnostic records remain invisible to plan recovery"
    );
}
