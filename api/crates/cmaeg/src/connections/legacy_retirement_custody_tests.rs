use super::*;
use crate::providers::{ProviderClient, ProviderRegistry};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    response::Response,
    routing::delete,
    Router,
};
use http_body_util::BodyExt;
use std::{future::Future, sync::atomic::AtomicUsize};
use tower::ServiceExt;

struct Retirement {
    router: Router,
    path: String,
    unbinds: Arc<AtomicUsize>,
    provider: tokio::task::JoinHandle<()>,
}

impl Retirement {
    async fn new(app: &Native) -> Self {
        let (mut definition, channel_id) = app.reserve_child().await;
        binding::activate(&app.service.management, &definition, &channel_id)
            .await
            .unwrap();
        definition.creation = None;
        definition.secrets.provider_credential =
            Some(cmaeg_core::channel::Secret::new("fixture-provider-key"));
        app.fixture.retain_channel(&definition).await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let registry = ProviderRegistry::parse(
            &serde_json::json!([{
                "slug": definition.provider.slug,
                "base_url": base,
                "kind": "responses_http",
                "paths": {
                    "agent_card": "/.well-known/agent-card.json",
                    "binding": "/fixture-bindings/{binding_id}",
                    "conversation": "/fixture-bindings/{binding_id}/conversations/{conversation_id}",
                    "responses": "/api/v2/responses"
                },
                "max_inflight": 8
            }])
            .to_string(),
        )
        .unwrap();
        let mut state = app.service.management.clone();
        state.records = Arc::new(app.fixture.records().with_channel_mutation_custody(true));
        state.providers = Arc::new(
            ProviderClient::new(registry, Duration::from_secs(2), Duration::from_secs(2), 8)
                .unwrap(),
        );
        let unbinds = Arc::new(AtomicUsize::new(0));
        let count = unbinds.clone();
        let provider = Router::new().route(
            "/fixture-bindings/{binding_id}",
            delete(move || {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    StatusCode::NO_CONTENT
                }
            }),
        );
        Self {
            router: crate::http::management_router(state),
            path: format!("/v1/channel-records/{}/management", definition.record_id),
            unbinds,
            provider: tokio::spawn(async move { axum::serve(listener, provider).await.unwrap() }),
        }
    }

    fn request(&self, method: Method, query: &str) -> impl Future<Output = Response> + 'static {
        let router = self.router.clone();
        let request = Request::builder()
            .method(method)
            .uri(format!("{}{query}", self.path))
            .header("authorization", "Bearer fixture-alice-bearer")
            .body(Body::empty())
            .unwrap();
        async move { router.oneshot(request).await.unwrap() }
    }

    fn delete(&self) -> impl Future<Output = Response> + 'static {
        self.request(Method::DELETE, "?expected_version=1")
    }

    async fn assert_state(&self, state: &str) {
        let response = self.request(Method::GET, "").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value["record"]["retirement"],
            serde_json::json!({"expected_version": 1, "state": state})
        );
    }
}

impl Drop for Retirement {
    fn drop(&mut self) {
        self.provider.abort();
    }
}

#[tokio::test]
async fn empty_webhook_lost_clear_ack_allows_record_delete_to_resume_without_resending() {
    let app = Native::new().await;
    let retirement = Retirement::new(&app).await;
    app.peer.webhook.lock().unwrap().clear();
    app.peer.pause_read.store(true, Ordering::SeqCst);

    let request = tokio::spawn(retirement.delete());
    tokio::time::timeout(Duration::from_secs(2), app.peer.read_started.notified())
        .await
        .unwrap();
    // The cleanup claim already committed before the platform read. The next
    // connection write clears that claim but its acknowledgement is lost.
    app.fixture.lose_put_ack_for(&app.record.id);
    app.peer.read_resume.notify_one();
    let refused = tokio::time::timeout(Duration::from_secs(2), request)
        .await
        .unwrap()
        .unwrap();
    assert!(refused.status().is_server_error());
    assert!(app.stored().await.0.webhook_lease.is_none());
    assert_eq!(retirement.unbinds.load(Ordering::SeqCst), 0);
    retirement.assert_state("pending").await;

    assert_eq!(retirement.delete().await.status(), StatusCode::NO_CONTENT);
    assert_eq!(retirement.delete().await.status(), StatusCode::NOT_FOUND);
    assert!(!app.methods().iter().any(|method| method == "deleteWebhook"));
    assert_eq!(retirement.unbinds.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn sent_webhook_delete_with_unknown_outcome_refuses_exact_record_retry() {
    let app = Native::new().await;
    let retirement = Retirement::new(&app).await;
    app.peer.lose_delete_reply.store(true, Ordering::SeqCst);

    assert!(retirement.delete().await.status().is_server_error());
    assert!(app.stored().await.0.webhook_lease.is_some());
    retirement.assert_state("cleanup_unconfirmed").await;

    assert_eq!(retirement.delete().await.status(), StatusCode::CONFLICT);
    assert_eq!(
        app.methods()
            .iter()
            .filter(|method| method.as_str() == "deleteWebhook")
            .count(),
        1
    );
    assert_eq!(retirement.unbinds.load(Ordering::SeqCst), 0);
}
