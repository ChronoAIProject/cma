//! Authentication and browser-boundary checks for the composed AG-UI routes.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode},
    middleware,
    routing::get,
    Json, Router,
};
use layer2_core::AttachmentClaim;
use tower::ServiceExt;

use super::{
    assert_header_list_contains, assert_json, assert_problem, create_ready_conversation_session,
    preflight_request, test_surface, with_cors, TestSurface, LISTED_ORIGIN,
};
use crate::auth::persistence::MemoryAuthStore;
use crate::auth::{self, AuthMode, AuthState};

const BEARER_FIXTURE: &str = "agui-route-fixture";

async fn auth_metadata() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "authorization_endpoint": "https://identity.example/authorize",
        "token_endpoint": "https://identity.example/token",
        "userinfo_endpoint": "https://identity.example/userinfo"
    }))
}

async fn auth_userinfo(headers: HeaderMap) -> (StatusCode, Json<serde_json::Value>) {
    let supplied = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let expected = format!("Bearer {BEARER_FIXTURE}");
    if supplied != Some(expected.as_str()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "invalid_token" })),
        );
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "sub": "subject-contract",
            "permissions": ["cma:session:use"]
        })),
    )
}

async fn provider_auth() -> (Arc<AuthState>, tokio::task::JoinHandle<()>) {
    let provider = Router::new()
        .route("/.well-known/openid-configuration", get(auth_metadata))
        .route("/userinfo", get(auth_userinfo));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("mock identity provider binds");
    let base_url = format!(
        "http://{}",
        listener.local_addr().expect("mock provider has an address")
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, provider)
            .await
            .expect("mock identity provider serves");
    });
    let auth = AuthState::with_store(
        AuthMode::Provider {
            base_url,
            client_id: "agui-route-client".into(),
            client_secret: "fixture".into(),
            public_url: "https://cma.example".into(),
        },
        None,
        Arc::new(MemoryAuthStore::default()),
    );
    (auth, server)
}

#[tokio::test]
async fn session_stream_accepts_a_provider_verified_bearer() {
    let TestSurface {
        app,
        sessions,
        caller,
        ..
    } = test_surface(false);
    let id = create_ready_conversation_session(&sessions, &caller).await;
    let (auth_state, provider) = provider_auth().await;
    let app = with_cors(
        app.layer(middleware::from_fn_with_state(
            auth_state,
            auth::attach_subject,
        )),
        vec![HeaderValue::from_static(LISTED_ORIGIN)],
    );

    let response = app
        .oneshot(
            Request::get(format!(
                "/api/v1/agui/events?thread_id={}&follow=false",
                id.0
            ))
            .header(header::ORIGIN, LISTED_ORIGIN)
            .header(header::AUTHORIZATION, format!("Bearer {BEARER_FIXTURE}"))
            .body(Body::empty())
            .expect("request builds"),
        )
        .await
        .expect("router answers");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&HeaderValue::from_static("text/event-stream"))
    );
    assert_eq!(
        response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&HeaderValue::from_static(LISTED_ORIGIN))
    );
    provider.abort();
}

#[tokio::test]
async fn session_stream_without_a_credential_returns_the_coded_401() {
    let TestSurface {
        app,
        sessions,
        caller,
        ..
    } = test_surface(false);
    let id = create_ready_conversation_session(&sessions, &caller).await;

    let response = app
        .oneshot(
            Request::get(format!(
                "/api/v1/agui/events?thread_id={}&follow=false",
                id.0
            ))
            .body(Body::empty())
            .expect("request builds"),
        )
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::UNAUTHORIZED).await;
    assert_eq!(body["code"], "not_authenticated");
}

#[tokio::test]
async fn capability_declaration_remains_public() {
    let response = test_surface(false)
        .app
        .oneshot(
            Request::get("/api/v1/agui/capabilities")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let body = assert_json(response, StatusCode::OK).await;
    assert_eq!(body["protocol"], "ag-ui");
    assert!(
        body.get("threadId").is_none() && body.get("runId").is_none(),
        "public interface discovery must carry no session authority: {body}"
    );
}

#[tokio::test]
async fn a_live_terminal_ticket_does_not_authenticate_the_session_stream() {
    let TestSurface {
        app,
        sessions,
        tickets,
        caller,
        ..
    } = test_surface(false);
    let id = create_ready_conversation_session(&sessions, &caller).await;
    let issued = tickets
        .issue(&caller, &id, AttachmentClaim::Exclusive)
        .await
        .expect("terminal ticket persists");

    let response = app
        .oneshot(
            Request::get(format!(
                "/api/v1/agui/events?thread_id={}&follow=false&ticket={}",
                id.0, issued.value
            ))
            .body(Body::empty())
            .expect("request builds"),
        )
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::UNAUTHORIZED).await;
    assert_eq!(body["code"], "not_authenticated");
    assert!(
        tickets
            .redeem(&issued.value, &id)
            .await
            .expect("terminal ticket loads")
            .is_some(),
        "an AG-UI request must neither accept nor consume a terminal ticket"
    );
}

#[tokio::test]
async fn listed_origin_allows_the_bearer_stream_preflight() {
    let app = with_cors(
        test_surface(false).app,
        vec![HeaderValue::from_static(LISTED_ORIGIN)],
    );
    let response = app
        .oneshot(preflight_request(
            "/api/v1/agui/events?thread_id=sesn_cross_origin",
            LISTED_ORIGIN,
            Method::GET,
            Some("authorization,last-event-id"),
        ))
        .await
        .expect("router answers");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&HeaderValue::from_static(LISTED_ORIGIN))
    );
    assert_eq!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS),
        Some(&HeaderValue::from_static("true"))
    );
    assert_header_list_contains(
        response.headers(),
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        "authorization",
    );
    assert_header_list_contains(
        response.headers(),
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        "last-event-id",
    );
    assert_header_list_contains(
        response.headers(),
        header::ACCESS_CONTROL_ALLOW_METHODS,
        "GET",
    );
}
