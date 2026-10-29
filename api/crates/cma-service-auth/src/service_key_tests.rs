use super::*;
use axum::{Form, Router, extract::State, http::StatusCode, response::IntoResponse, routing::post};
use serde_json::json;
use std::{
    collections::HashMap,
    future::IntoFuture,
    sync::{Arc, Mutex},
};

const SERVICE: &str = "00000000-0000-4000-8000-000000000001";

fn auth() -> ServiceKeyAuth {
    ServiceKeyAuth::new(
        NyxIdAuth::new(
            "https://identity.example",
            crate::Service::EventGateway,
            "client",
            "unused".into(),
        )
        .unwrap(),
        SERVICE,
    )
    .unwrap()
}

fn evidence() -> Value {
    json!({"active":true,"iss":"https://identity.example","aud":SERVICE,
        "sub":"owner","token_type":"api_key","scope":"read proxy","iat":10,"jti":"parent-key-id",
        "permissions":["cma-eg:channels:read"]})
}

#[test]
fn verified_nonexpiring_keys_preserve_optional_expiry_and_require_permissions() {
    let claim = auth().validate(&evidence(), 100).unwrap();
    assert_eq!(claim.expires_at, None);
    assert_eq!(claim.issued_at, 10);
    assert_eq!(claim.key_id, "parent-key-id");
    claim.require("cma-eg:channels:read").unwrap();
    assert_eq!(
        claim.require("cma-eg:channels:delete"),
        Err(AuthError::Forbidden)
    );
    let mut value = evidence();
    value["exp"] = json!(200);
    assert_eq!(auth().validate(&value, 100).unwrap().expires_at, Some(200));
    value["permissions"] = json!(["cma-eg:admin"]);
    auth()
        .validate(&value, 100)
        .unwrap()
        .require("cma-eg:channels:delete")
        .unwrap();
}

#[test]
fn missing_wrong_malformed_or_inactive_evidence_is_never_owner_authority() {
    for (field, bad) in [
        ("active", json!(false)),
        ("sub", json!(" ")),
        ("iss", json!("https://other.example")),
        ("aud", json!("cma-event-gateway")),
        ("aud", json!([SERVICE])),
        ("token_type", json!("access")),
        ("scope", json!("read")),
        ("resource", json!([])),
        ("iat", json!(101)),
        ("iat", json!(-1)),
        ("iat", json!("10")),
        ("iat", json!(null)),
        ("jti", json!("")),
        ("exp", json!(null)),
        ("exp", json!(100)),
        ("exp", json!("200")),
        ("permissions", json!(null)),
        ("permissions", json!([false])),
    ] {
        let mut value = evidence();
        value[field] = bad;
        assert!(
            auth().validate(&value, 100).is_err(),
            "accepted {field}: {value}"
        );
    }
    for field in [
        "active",
        "sub",
        "iss",
        "aud",
        "token_type",
        "scope",
        "iat",
        "jti",
        "permissions",
    ] {
        let mut value = evidence();
        value.as_object_mut().unwrap().remove(field);
        assert!(
            auth().validate(&value, 100).is_err(),
            "accepted missing {field}"
        );
    }
}

#[tokio::test]
async fn key_introspection_uses_confidential_service_contract_and_never_caches() {
    type Calls = Arc<Mutex<Vec<HashMap<String, String>>>>;
    async fn introspect(
        State(calls): State<Calls>,
        Form(form): Form<HashMap<String, String>>,
    ) -> impl IntoResponse {
        calls.lock().unwrap().push(form);
        axum::Json(json!({"active":false}))
    }
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/oauth/introspect", post(introspect))
                .with_state(calls.clone()),
        )
        .into_future(),
    );
    let secret = uuid::Uuid::new_v4().to_string();
    let token = format!("nyx_{}", uuid::Uuid::new_v4());
    let auth = ServiceKeyAuth::new(
        NyxIdAuth::new(
            &issuer,
            crate::Service::EventGateway,
            "test-client",
            secret.clone(),
        )
        .unwrap(),
        SERVICE,
    )
    .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
    for _ in 0..2 {
        assert!(matches!(
            auth.authenticate(&headers).await,
            Err(AuthError::Unauthenticated)
        ));
    }
    let seen = calls.lock().unwrap();
    assert_eq!(seen.len(), 2);
    for form in seen.iter() {
        assert_eq!(form.len(), 5);
        assert_eq!(form["token"], token);
        assert_eq!(form["token_type_hint"], "api_key");
        assert_eq!(form["service_id"], SERVICE);
        assert_eq!(form["client_id"], "test-client");
        assert_eq!(form["client_secret"], secret);
    }
    assert!(!format!("{auth:?}").contains(&secret));
    task.abort();
}

#[tokio::test]
async fn duplicate_authorization_and_nonkeys_fail_before_network() {
    let mut headers = HeaderMap::new();
    headers.append(AUTHORIZATION, "Bearer nyx_placeholder".parse().unwrap());
    headers.append(AUTHORIZATION, "Bearer other".parse().unwrap());
    assert!(matches!(
        auth().authenticate(&headers).await,
        Err(AuthError::Unauthenticated)
    ));
    headers.clear();
    headers.insert(AUTHORIZATION, "Bearer access-token".parse().unwrap());
    assert!(matches!(
        auth().authenticate(&headers).await,
        Err(AuthError::Unauthenticated)
    ));
}

#[tokio::test]
async fn introspection_http_failure_is_distinct_from_inactive_key() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(
        axum::serve(
            listener,
            Router::new().route(
                "/oauth/introspect",
                post(|| async { StatusCode::UNAUTHORIZED }),
            ),
        )
        .into_future(),
    );
    let auth = ServiceKeyAuth::new(
        NyxIdAuth::new(
            &issuer,
            crate::Service::EventGateway,
            "client",
            "unused".into(),
        )
        .unwrap(),
        SERVICE,
    )
    .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        format!("Bearer nyx_{}", uuid::Uuid::new_v4())
            .parse()
            .unwrap(),
    );
    assert!(matches!(
        auth.authenticate(&headers).await,
        Err(AuthError::Unavailable)
    ));
    task.abort();
}
