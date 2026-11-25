use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use layer2_core::{
    account_deletion::OwnerSessionAuthority,
    legal_consent::{LegalConsentService, LegalTerms, MemoryLegalConsentStore},
    Authorization, Capability, SubjectId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

fn router(configured: bool) -> Router {
    let terms = configured.then(|| {
        LegalTerms::new(
            "2026-11-30",
            "https://legal.example.test/terms",
            "https://legal.example.test/privacy",
        )
        .unwrap()
    });
    super::legal_consent_router(Arc::new(LegalConsentService::new(
        terms,
        Arc::new(MemoryLegalConsentStore::default()),
    )))
}

enum Caller {
    Owner,
    NonHuman,
    Binding,
    Anonymous,
}

fn request(method: Method, caller: Caller, body: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri("/api/v1/account/legal-consent");
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let mut request = builder
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_owned())))
        .unwrap();
    if !matches!(caller, Caller::Anonymous) {
        request
            .extensions_mut()
            .insert(crate::AuthenticatedSubject(SubjectId::new("owner")));
        request
            .extensions_mut()
            .insert(Authorization::new([Capability::UseOwnSandboxes]));
    }
    if matches!(caller, Caller::Owner | Caller::Binding) {
        request
            .extensions_mut()
            .insert(OwnerSessionAuthority::verified_personal(SubjectId::new(
                "owner",
            )));
    }
    if matches!(caller, Caller::Binding) {
        request
            .extensions_mut()
            .insert(crate::AuthMethod::BindingKey {
                binding_id: "binding".into(),
            });
    }
    request
}

async fn send(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 16_384).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn owner_reads_then_records_the_current_version_once() {
    let router = router(true);
    let (status, body) = send(&router, request(Method::GET, Caller::Owner, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({
            "version": "2026-11-30",
            "terms_url": "https://legal.example.test/terms",
            "privacy_url": "https://legal.example.test/privacy",
            "accepted_at": null,
        })
    );

    let accept = r#"{"version":"2026-11-30"}"#;
    let (status, accepted) = send(&router, request(Method::PUT, Caller::Owner, Some(accept))).await;
    assert_eq!(status, StatusCode::OK);
    let at = accepted["accepted_at"].as_str().unwrap().to_owned();
    let (_, replay) = send(&router, request(Method::PUT, Caller::Owner, Some(accept))).await;
    assert_eq!(replay["accepted_at"], at.as_str());
    let (_, read) = send(&router, request(Method::GET, Caller::Owner, None)).await;
    assert_eq!(read, accepted);
}

#[tokio::test]
async fn refusals_are_typed_and_private() {
    let router = router(true);
    let cases = [
        (
            request(
                Method::PUT,
                Caller::Owner,
                Some(r#"{"version":"2026-01-01"}"#),
            ),
            StatusCode::CONFLICT,
            "legal_terms_version_mismatch",
        ),
        (
            request(Method::GET, Caller::NonHuman, None),
            StatusCode::FORBIDDEN,
            "legal_consent_requires_owner_session",
        ),
        (
            request(
                Method::PUT,
                Caller::Binding,
                Some(r#"{"version":"2026-11-30"}"#),
            ),
            StatusCode::FORBIDDEN,
            "legal_consent_requires_owner_session",
        ),
        (
            request(Method::GET, Caller::Anonymous, None),
            StatusCode::UNAUTHORIZED,
            "not_authenticated",
        ),
        (
            request(
                Method::PUT,
                Caller::Owner,
                Some(r#"{"version":"2026-11-30","x":1}"#),
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_json",
        ),
        (
            request(Method::PUT, Caller::Owner, Some("{")),
            StatusCode::BAD_REQUEST,
            "invalid_json",
        ),
    ];
    for (request, status, code) in cases {
        let (actual, body) = send(&router, request).await;
        assert_eq!((actual, body["code"].as_str()), (status, Some(code)));
    }
    let mut unlabeled = request(Method::PUT, Caller::Owner, None);
    *unlabeled.body_mut() = Body::from(r#"{"version":"2026-11-30"}"#);
    let (status, body) = send(&router, unlabeled).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(body["code"], "json_content_type_required");
    let (_, read) = send(&router, request(Method::GET, Caller::Owner, None)).await;
    assert_eq!(read["accepted_at"], Value::Null);
}

#[tokio::test]
async fn unconfigured_deployments_answer_not_configured() {
    let router = router(false);
    for request in [
        request(Method::GET, Caller::Owner, None),
        request(Method::PUT, Caller::Owner, Some(r#"{"version":"1"}"#)),
    ] {
        let (status, body) = send(&router, request).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "legal_terms_not_configured");
    }
}
