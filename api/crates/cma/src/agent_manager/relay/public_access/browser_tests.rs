use super::super::{Relay, PROOF};
use super::fixture::{fixture, Fixture};
use crate::auth::relay::tests::Fixture as Browser;
use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    Router,
};
use layer2_core::{
    agent_ownership::{
        test_support::MemoryOwnershipStore, Member, OwnershipCoordinator, PROTOCOL_VERSION,
    },
    sandbox::DurableAgentUpdateRecorder,
    AgentId,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tower::ServiceExt;

const PREVIEW: &str = "/api/v1/agents/agt-public/files/content?path=sample";

async fn replica(
    store: Arc<MemoryOwnershipStore>,
    resources: &Fixture,
    browser: &Browser,
    instance: &str,
    address: String,
) -> (Router, Arc<OwnershipCoordinator>) {
    let coordinator = OwnershipCoordinator::register(
        store,
        Member {
            instance_id: instance.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: address,
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 16,
        },
    )
    .await
    .unwrap();
    let updates = DurableAgentUpdateRecorder::new(resources.store.clone());
    let relay = Relay::new(
        coordinator.clone(),
        &updates,
        ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"synthetic-file-preview-peer-key"),
    )
    .unwrap();
    relay
        .install_public_access(resources.policy.clone())
        .unwrap();
    let router = relay.authenticated_routes(
        Router::new().route(
            "/api/v1/agents/{id}/files/content",
            axum::routing::get(move || async move { StatusCode::OK }),
        ),
        browser.auth.clone(),
    );
    (Relay::file_preview_identity_route(router), coordinator)
}

async fn preview(router: &Router, browser: &Browser, verifications: usize, status: StatusCode) {
    let mut request = Request::builder().uri(PREVIEW).body(Body::empty()).unwrap();
    *request.headers_mut() = browser.headers.clone();
    let before = browser.provider_calls();
    assert_eq!(
        router.clone().oneshot(request).await.unwrap().status(),
        status
    );
    assert_eq!(
        browser.provider_calls(),
        (before.0 + verifications, before.1 + verifications)
    );
}

#[tokio::test]
async fn file_preview_peer_and_recovery_each_verify_independently() {
    let browser = Browser::new(true).await;
    let resources = fixture(browser.auth.clone(), "agt-public", "owner").await;
    let store = Arc::new(MemoryOwnershipStore::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let (owner, coordinator) = replica(store.clone(), &resources, &browser, "owner", address).await;
    let _authority = coordinator
        .acquire(&AgentId("agt-public".into()))
        .await
        .unwrap()
        .unwrap();
    preview(
        &crate::with_request_context(owner.clone()),
        &browser,
        1,
        StatusCode::OK,
    )
    .await;

    let handoff = Arc::new(AtomicBool::new(false));
    let owner = owner.layer(axum::middleware::from_fn_with_state(
        (handoff.clone(), coordinator),
        |State((handoff, coordinator)): State<(Arc<AtomicBool>, Arc<OwnershipCoordinator>)>,
         request: Request,
         next: Next| async move {
            if request.headers().contains_key(PROOF) && handoff.load(Ordering::SeqCst) {
                coordinator.begin_drain().await;
            }
            next.run(request).await
        },
    ));
    let server = tokio::spawn(async move {
        axum::serve(listener, crate::with_request_context(owner))
            .await
            .unwrap();
    });
    let (ingress, _) = replica(
        store,
        &resources,
        &browser,
        "ingress",
        "http://127.0.0.1:1".into(),
    )
    .await;
    let ingress = crate::with_request_context(ingress);
    preview(&ingress, &browser, 3, StatusCode::OK).await;
    handoff.store(true, Ordering::SeqCst);
    preview(&ingress, &browser, 4, StatusCode::OK).await;
    server.abort();
}
