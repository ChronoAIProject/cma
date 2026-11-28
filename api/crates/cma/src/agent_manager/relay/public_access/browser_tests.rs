use super::super::{
    authentication::{self, Authentication},
    file_identity::{ProjectedIdentity, HEADER},
    file_read, file_write, Relay, PROOF,
};
use super::fixture::{fixture, Fixture};
use crate::auth::relay::tests::Fixture as Browser;
use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    middleware::Next,
    Router,
};
use layer2_core::{
    agent_ownership::{
        test_support::MemoryOwnershipStore, Member, OwnershipCoordinator, OwnershipToken,
        PROTOCOL_VERSION,
    },
    sandbox::DurableAgentUpdateRecorder,
    AgentId,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tower::ServiceExt;

const PREVIEW: &str = "/api/v1/agents/agt-public/files/content?path=sample";
const TEST_SIGNING_KEY: &[u8] = b"synthetic-file-preview-peer-key";

#[derive(Clone)]
struct OwnerFixture {
    handoff: Arc<AtomicBool>,
    coordinator: Arc<OwnershipCoordinator>,
    wire: Arc<Mutex<Option<HeaderMap>>>,
}

async fn replica(
    store: Arc<MemoryOwnershipStore>,
    resources: &Fixture,
    browser: &Browser,
    instance: &str,
    address: String,
    peer_identity: bool,
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
        ring::hmac::Key::new(ring::hmac::HMAC_SHA256, TEST_SIGNING_KEY),
    )
    .unwrap();
    relay
        .install_public_access(resources.policy.clone())
        .unwrap();
    if peer_identity {
        relay.enable_file_preview_peer_identity();
    }
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
    request
        .headers_mut()
        .insert(HEADER, "synthetic-client-claim".parse().unwrap());
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
    verify_peer_flow(false, false, 3, 4).await;
}

#[tokio::test]
async fn file_preview_owner_skips_provider_but_recovery_verifies() {
    verify_peer_flow(true, true, 1, 2).await;
}

#[tokio::test]
async fn file_preview_sender_interoperates_with_disabled_receiver() {
    verify_peer_flow(true, false, 3, 4).await;
}

async fn verify_peer_flow(sender: bool, receiver: bool, forwarded: usize, recovered: usize) {
    let browser = Browser::new(true).await;
    let resources = fixture(browser.auth.clone(), "agt-public", "owner").await;
    let store = Arc::new(MemoryOwnershipStore::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let (owner, coordinator) = replica(
        store.clone(),
        &resources,
        &browser,
        "owner",
        address,
        receiver,
    )
    .await;
    let authority = coordinator
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
    let wire = Arc::new(Mutex::new(None::<HeaderMap>));
    let owner = owner.layer(axum::middleware::from_fn_with_state(
        OwnerFixture {
            handoff: handoff.clone(),
            coordinator,
            wire: wire.clone(),
        },
        |State(owner): State<OwnerFixture>, request: Request, next: Next| async move {
            if request.headers().contains_key(PROOF) {
                *owner.wire.lock().unwrap() = Some(request.headers().clone());
                if owner.handoff.load(Ordering::SeqCst) {
                    owner.coordinator.begin_drain().await;
                }
            }
            next.run(request).await
        },
    ));
    let owner = crate::with_request_context(owner);
    let server = tokio::spawn({
        let owner = owner.clone();
        async move { axum::serve(listener, owner).await.unwrap() }
    });
    let (ingress, _) = replica(
        store,
        &resources,
        &browser,
        "ingress",
        "http://127.0.0.1:1".into(),
        sender,
    )
    .await;
    let ingress = crate::with_request_context(ingress);
    preview(&ingress, &browser, forwarded, StatusCode::OK).await;
    let signed = wire.lock().unwrap().clone().unwrap();
    if sender {
        assert_ne!(signed[HEADER], "synthetic-client-claim");
    } else {
        assert!(!signed.contains_key(HEADER));
    }
    if receiver {
        exercise_peer_refusals(&owner, &browser, &signed, &authority.token).await;
    }
    handoff.store(true, Ordering::SeqCst);
    preview(&ingress, &browser, recovered, StatusCode::OK).await;
    server.abort();
}

async fn exercise_peer_refusals(
    owner: &Router,
    browser: &Browser,
    signed: &HeaderMap,
    token: &OwnershipToken,
) {
    let before = browser.provider_calls();
    let uri: Uri = PREVIEW.parse().unwrap();
    let signer = Authentication::new(ring::hmac::Key::new(
        ring::hmac::HMAC_SHA256,
        TEST_SIGNING_KEY,
    ));
    let sign = |sender: &str, token: &OwnershipToken, mut headers: HeaderMap| {
        let projection = ProjectedIdentity::from_test_header(headers.remove(HEADER).unwrap());
        signer
            .signed_file_preview_headers(sender, token, &uri, headers, projection)
            .unwrap()
    };
    let sign_legacy = |headers: HeaderMap, body: &[u8]| {
        signer
            .signed_headers("ingress", token, &Method::GET, &uri, headers, body)
            .unwrap()
    };
    // The predecessor forwards all otherwise eligible x-cma headers except
    // these response markers. Its generic signer issues a file handoff nonce.
    let mut legacy = HeaderMap::new();
    for (name, value) in signed {
        if !matches!(
            name.as_str(),
            file_read::HANDOFF_HEADER | file_write::NOT_ENTERED_HEADER
        ) && authentication::forwarded(name.as_str())
        {
            legacy.append(name, value.clone());
        }
    }
    assert!(legacy.contains_key(HEADER));
    let legacy_claim = sign_legacy(legacy.clone(), &[]);
    legacy.remove(HEADER);
    let legacy_request = sign_legacy(legacy, &[]);
    let altered = sign("ingress", token, signed.clone());
    let body = sign_legacy(signed.clone(), b"unexpected");
    let unknown = sign("absent-member", token, signed.clone());
    let mut wrong = token.clone();
    wrong.epoch += 1;
    let stale_epoch = sign("ingress", &wrong, signed.clone());
    let mut invalid = signed.clone();
    invalid.insert(PROOF, "malformed".parse().unwrap());
    let mut malformed_projection = signed.clone();
    malformed_projection.insert(HEADER, "invalid".parse().unwrap());
    let malformed_projection = sign("ingress", token, malformed_projection);
    for (headers, path, body, expected) in [
        (signed.clone(), PREVIEW, "", StatusCode::FORBIDDEN),
        (legacy_claim, PREVIEW, "", StatusCode::FORBIDDEN),
        (
            altered,
            "/api/v1/agents/agt-public/files/content?path=other",
            "",
            StatusCode::FORBIDDEN,
        ),
        (body, PREVIEW, "unexpected", StatusCode::FORBIDDEN),
        (unknown, PREVIEW, "", StatusCode::FORBIDDEN),
        (stale_epoch, PREVIEW, "", StatusCode::SERVICE_UNAVAILABLE),
        (invalid, PREVIEW, "", StatusCode::FORBIDDEN),
        (malformed_projection, PREVIEW, "", StatusCode::FORBIDDEN),
    ] {
        let mut request = Request::builder().uri(path).body(Body::from(body)).unwrap();
        *request.headers_mut() = headers;
        assert_eq!(
            owner.clone().oneshot(request).await.unwrap().status(),
            expected
        );
    }
    let mut request = Request::builder().uri(PREVIEW).body(Body::empty()).unwrap();
    *request.headers_mut() = sign("ingress", token, signed.clone());
    browser.store.set_unavailable(true);
    let response = owner.clone().oneshot(request).await.unwrap();
    browser.store.set_unavailable(false);
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(browser.provider_calls(), before);
    let mut request = Request::builder().uri(PREVIEW).body(Body::empty()).unwrap();
    *request.headers_mut() = legacy_request;
    assert_eq!(
        owner.clone().oneshot(request).await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(browser.provider_calls(), (before.0 + 2, before.1 + 2));
}
