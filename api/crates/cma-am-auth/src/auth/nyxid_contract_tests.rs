//! Real client constructors, synthetic loopback authority, no external NyxID.
use super::*;
use crate::dependency_test_support::{assert_deadline, finish, Exchange, Peer};
use axum::http::{header, Method};
use serde_json::{json, Value};
use std::time::UNIX_EPOCH;

use super::test_support::contract_auth as auth;

fn metadata(origin: &str) -> Value {
    json!({
        "authorization_endpoint":format!("{origin}/oauth/authorize"),
        "token_endpoint":format!("{origin}/oauth/token"),
        "userinfo_endpoint":format!("{origin}/oauth/userinfo"),
        "introspection_endpoint":format!("{origin}/oauth/introspect")
    })
}

fn assert_identity_request(request: &Exchange) {
    match request.uri.path() {
        "/oauth/userinfo" => {
            assert_eq!(request.method, Method::GET);
            assert_eq!(
                request.headers[header::AUTHORIZATION],
                "Bearer contract-bearer"
            );
            assert!(request.body.is_empty());
        }
        "/oauth/introspect" => {
            assert_eq!(request.method, Method::POST);
            let fields: std::collections::BTreeMap<_, _> =
                url::form_urlencoded::parse(&request.body)
                    .into_owned()
                    .collect();
            assert_eq!(
                fields,
                std::collections::BTreeMap::from([
                    ("token".into(), "contract-bearer".into()),
                    ("token_type_hint".into(), "access_token".into()),
                    ("client_id".into(), "contract-client".into()),
                    ("client_secret".into(), "contract-client-fixture".into()),
                ])
            );
            assert!(!request.headers.contains_key(header::AUTHORIZATION));
        }
        path => panic!("unexpected synthetic provider path: {path}"),
    }
    assert!(request.uri.query().is_none());
}

async fn pair(peer: &mut Peer) -> [Exchange; 2] {
    let a = peer.next().await;
    let b = peer.next().await;
    assert_identity_request(&a);
    assert_identity_request(&b);
    assert_ne!(a.uri.path(), b.uri.path());
    [a, b]
}

fn verify(
    auth: Arc<AuthState>,
) -> tokio::task::JoinHandle<Result<Option<RequestAuth>, StoreError>> {
    tokio::spawn(async move { identity::request_for_bearer(&auth, "contract-bearer".into()).await })
}

#[tokio::test]
async fn discovery_identity_and_permission_contract_bind_one_credential() {
    let mut peer = Peer::start().await;
    let auth = auth(&peer.origin);
    let task = verify(auth);
    let discovery = peer.next().await;
    assert_eq!(discovery.method, Method::GET);
    assert_eq!(discovery.uri.path(), "/.well-known/openid-configuration");
    assert!(!discovery.headers.contains_key(header::AUTHORIZATION));
    discovery.json(metadata(&peer.origin));
    for request in pair(&mut peer).await {
        if request.uri.path() == "/oauth/userinfo" {
            request.json(json!({"sub":"contract-owner","name":"Fixture"}));
        } else {
            request.json(json!({"active":true,"sub":"contract-owner","permissions":[]}));
        }
    }
    let evidence = finish(task).await.unwrap().unwrap();
    assert_eq!(evidence.subject, SubjectId::new("contract-owner"));
    assert_eq!(peer.count(), 3);
}

#[tokio::test]
async fn identity_503_is_three_attempts_then_typed_unavailable_without_cached_success() {
    let mut peer = Peer::start().await;
    let auth = auth(&peer.origin);
    *auth.metadata.write().await = Some(serde_json::from_value(metadata(&peer.origin)).unwrap());
    let started = tokio::time::Instant::now();
    let task = verify(auth.clone());
    for _ in 0..3 {
        for request in pair(&mut peer).await {
            request.respond(503, "synthetic upstream failure");
        }
    }
    assert!(
        matches!(finish(task).await, Err(StoreError::Unavailable(code)) if code == "identity_provider_unavailable")
    );
    assert!(started.elapsed() <= Duration::from_millis(2500));
    assert_eq!(peer.count(), 6);
    // A new request must contact NyxID; failure is not a cached authorization.
    let task = verify(auth);
    for request in pair(&mut peer).await {
        request.json(json!({"active":true,"sub":"contract-owner","permissions":[]}));
    }
    assert!(finish(task).await.unwrap().is_some());
    assert_eq!(peer.count(), 8);
}

#[tokio::test]
async fn identity_total_deadline_cancels_pending_evidence_without_detached_retries() {
    let mut peer = Peer::start().await;
    let auth = auth(&peer.origin);
    *auth.metadata.write().await = Some(serde_json::from_value(metadata(&peer.origin)).unwrap());
    let task = verify(auth);
    let held = pair(&mut peer).await;
    tokio::time::pause();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(2500);
    tokio::time::advance(Duration::from_millis(2500)).await;
    assert!(matches!(
        finish(task).await,
        Err(StoreError::Unavailable(_))
    ));
    assert_deadline(deadline);
    tokio::time::advance(Duration::from_secs(60)).await;
    tokio::task::yield_now().await;
    assert_eq!(peer.count(), 2);
    drop(held);
}

fn service_auth(origin: &str) -> cma_service_auth::NyxIdAuth {
    cma_service_auth::NyxIdAuth::new(
        origin,
        cma_service_auth::Service::EventGateway,
        "contract-client",
        "contract-client-fixture".into(),
    )
    .unwrap()
}

fn verify_service(
    auth: cma_service_auth::NyxIdAuth,
) -> tokio::task::JoinHandle<Result<cma_service_auth::Principal, cma_service_auth::AuthError>> {
    tokio::spawn(async move {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "Bearer contract-bearer".parse().unwrap(),
        );
        auth.authenticate(&headers).await
    })
}

#[tokio::test]
async fn service_introspection_binds_issuer_resource_and_permission_and_does_not_retry_503() {
    for success in [true, false] {
        let mut peer = Peer::start().await;
        let task = verify_service(service_auth(&peer.origin));
        let request = peer.next().await;
        assert_identity_request(&request);
        assert_eq!(request.uri.path(), "/oauth/introspect");
        if success {
            request.json(
                json!({"active":true,"sub":"contract-owner","iss":peer.origin,
                "exp":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 300,
                "token_type":"access", "permissions":["cma-eg:channels:read"],
                "resource":[format!("{}/api/v1/proxy/s/cma-event-gateway", peer.origin)]}),
            );
            let principal = finish(task).await.unwrap();
            principal.require("cma-eg:channels:read").unwrap();
            assert_eq!(
                principal.require("cma-eg:channels:write"),
                Err(cma_service_auth::AuthError::Forbidden)
            );
        } else {
            request.respond(503, "synthetic unavailable");
            assert!(matches!(
                finish(task).await,
                Err(cma_service_auth::AuthError::Unavailable)
            ));
        }
        assert_eq!(peer.count(), 1);
    }
}

#[tokio::test]
async fn service_introspection_stall_is_typed_unavailable_at_four_seconds_without_retry() {
    for body in [false, true] {
        let mut peer = Peer::start().await;
        let task = verify_service(service_auth(&peer.origin));
        let request = peer.next().await;
        let held = if body {
            request.stall_body();
            None
        } else {
            Some(request)
        };
        tokio::time::pause();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
        tokio::time::advance(Duration::from_secs(4)).await;
        assert!(matches!(
            finish(task).await,
            Err(cma_service_auth::AuthError::Unavailable)
        ));
        assert_deadline(deadline);
        tokio::time::advance(Duration::from_secs(60)).await;
        tokio::task::yield_now().await;
        assert_eq!(peer.count(), 1);
        drop(held);
        tokio::time::resume();
    }
}
